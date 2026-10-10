#!/usr/bin/env python3
"""Checks that a clippy invocation cannot quietly lint less than it claims.

Two commands in this repository describe themselves as workspace-wide: the
clippy line in the `ritual` make target, and the Clippy step in the pull request
workflow. Both used to name no package selector at all, and the root manifest set
`default-members` to a single crate, so cargo selected that one crate for both.
Sixteen of the seventeen crates, and their test targets, were never linted by
either command. A `-D warnings` error in any of them could not fail either gate,
and neither reported that it had looked at nothing.

The check exists because a lint gate that cannot reach a crate cannot report one,
and nothing else in the tree would have noticed.

Rules, each read where the thing it describes lives:

1. Every clippy invocation in the Makefile, in a shell script, or in a workflow
   names what it selects. `--workspace` does. So does `-p`. So does a `$(VAR)`
   whose every definition names a workspace token. A line is refused when it
   carries `--exclude`, because that drops a crate out of a workspace run while
   still reading as one. `--all` is accepted and reported: cargo documents it as
   a deprecated alias for `--workspace`, and a check that blesses the old spelling
   lets it come back without a word.
2. The root manifest does not set `default-members` inside `[workspace]`. It is
   the switch that made an unqualified `cargo clippy` mean one crate -- in a
   developer's shell as much as in CI -- and no command line can undo it
   everywhere.
3. At least one clippy run exists, and a file that mentions clippy but runs none
   is a finding. Deleting the Clippy step and the recipe both leave zero
   invocations, and a check that counts only the lines it can see reports that as
   clean, which is the loss this repository has already made once: a target that
   stopped running a check while keeping its name.
4. Every clippy run reaches cargo through a bare name, and nothing on the line
   names a toolchain around it. A toolchain binary invoked by path --
   `$(CARGO)`, `$(RUSTUP_BIN)/cargo`, or an absolute path -- does not read
   `rust-toolchain.toml`, so the run lints with whichever compiler this host
   carries while the runner uses the one the repository pins. Neither does a
   bare `cargo` that arrives through `rustup run <toolchain>`, or with
   `RUSTUP_TOOLCHAIN`, `RUSTC`, `RUSTDOC` or `CARGO` set on the same line:
   rustup resolves a named toolchain and its environment override ahead of the
   toolchain file, and the other three tell cargo which compiler to run. In
   every one of these shapes the two gates disagree about the same tree, and the
   local one is the wrong one. A `$(NAME)` or `${NAME}` reference is the one way
   to write the executable that can still pass: it passes when every definition
   of that name in the Makefile is a bare `cargo`, and is refused when one is a
   path or when the file never defines the name.

A file this check cannot read is not a skip. A scan that declines to look cannot
report what it did not see, and that is the failure this repository has had three
times already.

What is not scanned, stated rather than left to be assumed:

- **Python.** A clippy run added to a file under `scripts/` would not be caught
  here. It would be caught by `make ritual` and by CI, and a check that tried to
  find it by scanning Python source would read this file's own docstring and its
  own fixtures as two more invocations, so the false positives cost more than the
  hole.
- **`scripts/gate.py`,** which is the one file that narrows a clippy run on the
  direct-cargo path. It used to strip `--workspace` and substitute the changed
  crates, so a workspace-wide command a contributor typed produced a run over a
  few crates published as the lint verdict. It now returns a clippy command that
  names `--workspace` or `--all` unchanged, and that refusal is covered by the
  gate's own self-test rather than by this file. `make lint` is still scoped on
  purpose, through `PACKAGES`, and this check reads that variable rather than
  objecting to it.

Usage:
  python3 scripts/lint_scope_check.py
  python3 scripts/lint_scope_check.py --self-test
  python3 scripts/lint_scope_check.py --list-scanned

Exit codes:
  0  every clippy run names what it selects and reaches cargo by a bare name
     through the shim, with nothing on the line overriding the toolchain, the
     manifest does not narrow by default, and at least one clippy run exists
  1  at least one finding, each naming the file and line
  2  usage error
  3  a file or directory this check had to read could not be read, so the clean
     parts of the run do not cover it
"""

from __future__ import annotations

import contextlib
import io
import re
import sys
import tempfile
from pathlib import Path

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

USAGE = "usage: lint_scope_check.py | --self-test | --list-scanned"

#: The root manifest, where `default-members` is rule 2.
MANIFEST = "Cargo.toml"

#: Files that must exist for this check to have read the whole surface. A missing
#: one is a finding rather than a pass, because a Makefile that had been renamed
#: would otherwise turn this check into a check of nothing.
REQUIRED = (MANIFEST, "Makefile")

#: The only spelling that satisfies rule 1. `--all` is a documented deprecated
#: alias, so it is accepted and reported rather than treated as correct.
SCOPE_TOKENS = frozenset({"--workspace"})

#: Deprecated spelling, mapped to what to write instead.
DEPRECATED_ALIASES = {"--all": "--workspace"}

#: The cargo executables that are reached by name, through the shim on PATH, and
#: therefore read `rust-toolchain.toml`. `cargo-clippy` and `clippy-driver` are in
#: the set because `_is_cargo` already calls them the same tool under other names,
#: and rustup installs all three as proxies. Anything else that `_is_cargo`
#: accepted -- a path, or a `$(VAR)` reference the file does not show to be one of
#: these -- is the rule's finding.
SHIM_EXECUTABLES = frozenset({"cargo", "cargo-clippy", "clippy-driver"})

#: Directories searched for shell scripts, each non-recursively. The list is
#: bounded on purpose: a recursive walk from the repository root would descend
#: into the build directory, where there are thousands of files and none of them
#: are commands anyone wrote.
SHELL_DIRS = ("scripts", "hooks.d")

#: Shell scripts in this repository that carry no extension, so the glob misses
#: them. `scripts/gate-shim` is the dispatcher that routes a bare `cargo clippy`
#: into the wrapper, which makes it the one shell file worth reading for a clippy
#: command.
SHELL_EXTRA = (
    "scripts/gate-shim",
    "scripts/git-hooks/pre-commit",
    "scripts/git-hooks/pre-merge-commit",
)

#: A make variable reference in either spelling, `$(NAME)` or `${NAME}`. Only
#: the paren form captures, so `findall` yields the names rather than pairs.
#: Both spellings are matched because `_is_cargo` unwraps both, and a check that
#: found `$(PACKAGES)` as a selector while reading `${PACKAGES}` as a literal
#: word would refuse a workspace-wide run for naming nothing.
VARIABLE = re.compile(r"\$\{[A-Za-z_][A-Za-z0-9_]*\}|\$\(([A-Za-z_][A-Za-z0-9_]*)\)")

#: A make variable name, used to tell a plain reference from a nested or
#: computed one that cannot be resolved to a name at all.
VARIABLE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

#: `NAME = value`, `NAME := value`, `NAME ::= value`, `NAME ?= value`, including a
#: target-specific assignment, whose leading whitespace this allows, and the
#: `export ` form of the same thing.
#:
#: Two gaps, both of which read a definition that make would honour as no
#: definition at all. The `export` prefix was not matched, and neither was the
#: plain `=`, so `export CARGO = cargo` defined nothing here: the reference on
#: the clippy line was refused for holding a toolchain path it never had. Make
#: takes the last assignment whichever form it is written in, and so does this.
ASSIGNMENT = re.compile(
    r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*(?:::=|:\??=|\?=|=)\s*(.*)$"
)

#: TOML accepts a quoted key, so `"default-members" = [...]` is the same key.
DEFAULT_MEMBERS = re.compile(r'^\s*"?default-members"?\s*=')

WORKSPACE_TABLE = re.compile(r"^\s*\[workspace\]\s*$")

TABLE_HEADER = re.compile(r"^\s*\[")


#: The environment variables that decide which toolchain a run uses rather than
#: which packages it selects, so a clippy line carrying one is refused. All four
#: are here because this Makefile exports or sets them, and an export reaches a
#: recipe whether or not the line itself names it.
TOOLCHAIN_OVERRIDES = frozenset({"RUSTUP_TOOLCHAIN", "RUSTC", "RUSTDOC", "CARGO"})

#: `NAME=value`, the shell's environment assignment, split on the first `=` so a
#: path that holds an `=` does not move the name.
ENV_ASSIGNMENT = re.compile(r"^(?P<name>[A-Za-z_][A-Za-z0-9_]*)=(?P<value>.*)$")


def _unwrap_variable(token: str) -> tuple[str | None, str]:
    """A make variable reference as `(name, bare text)`, or `(None, bare text)`.

    Make writes a variable in two spellings, `$(NAME)` and `${NAME}`. This check
    unwrapped only the first, so `$(CARGO) clippy` was recorded as an invocation
    and `${CARGO} clippy` was not recorded at all: the run count came out one
    short, neither rule reached the second line, and the gate reported the file
    clean. Both spellings are unwrapped here, and the name comes back with them
    so the executable can be judged against the file's own definitions of it.

    A reference that is not a plain name -- `$(RUSTUP_BIN)/cargo`, or a nested
    one -- comes back with no name, which is what the caller needs: there is no
    single definition to look up.
    """
    bare = token.strip("\"`'")
    name: str | None = None
    while True:
        if bare.startswith("$(") and bare.endswith(")"):
            inner = bare[2:-1].strip()
        elif bare.startswith("${") and bare.endswith("}"):
            inner = bare[2:-1].strip()
        else:
            return name, bare
        name = inner if VARIABLE_NAME.fullmatch(inner) else None
        bare = inner


def _is_cargo(token: str) -> bool:
    """Whether a token names a cargo executable.

    Make writes the executable as `$(CARGO)` or `$(RUSTUP_BIN)/cargo`, a workflow
    writes a bare `cargo`, so the token is unwrapped before it is compared rather
    than pattern-matched in place. The comparison folds case because make spells
    its variables in upper case and cargo ships in lower, and a comparison that
    missed `$(CARGO)` would read the Makefile's own clippy line as no invocation
    at all -- which is the whole failure this check exists for.
    `cargo-clippy` is accepted because it is the same tool under another name.
    """
    _, bare = _unwrap_variable(token)
    return bare.rsplit("/", 1)[-1].lower() in ("cargo", "cargo-clippy", "clippy-driver")


def _strip_comment(line: str) -> str:
    """Drop a whole-line comment, and a trailing one outside quotes.

    A trailing `#` used to survive into the token stream, so a recipe ending
    `# TODO: add --workspace` satisfied rule 1 while naming no selector -- which
    is exactly the half-finished line a contributor writes when they know the
    selector is missing. Trimming at the first `#` regardless of quoting would cut
    a command in half wherever the character appears inside a quoted argument, so
    the quote state is tracked. No cargo selector, lint flag or path in these
    commands contains a `#`, so a `#` outside quotes is a comment here.
    """
    stripped = line.strip()
    if stripped.startswith("#") or stripped.startswith("/*"):
        return ""
    quote = None
    for index, char in enumerate(line):
        if quote is not None:
            if char == quote:
                quote = None
        elif char in "'\"":
            quote = char
        elif char == "#":
            return line[:index]
    return line


def _logical_lines(text: str):
    """Yield (first line number, joined command), folding `\\` continuations.

    Make wraps a long recipe across lines. Read one line at a time, the second
    physical line of a wrapped recipe looked like a command that named no
    packages, and the check refused a wrapped workspace-wide lint.
    """
    buffer = ""
    start = 0
    for number, raw in enumerate(text.splitlines(), start=1):
        line = _strip_comment(raw)
        if not buffer:
            start = number
        body = line.rstrip()
        if body.endswith("\\"):
            buffer += body[:-1] + " "
            continue
        buffer += line
        if buffer.strip():
            yield start, buffer
        buffer = ""
    if buffer.strip():
        yield start, buffer


def clippy_invocations(text: str) -> list[tuple[int, str]]:
    """Every command that runs clippy through cargo, as (line number, command).

    A cargo token is required as well as the word clippy, which is what keeps a
    dispatch arm such as `test | nextest | llvm-cov | clippy) heavy=1` from
    reading as an invocation.
    """
    found: list[tuple[int, str]] = []
    for number, joined in _logical_lines(text):
        tokens = joined.split()
        start = next((i for i, token in enumerate(tokens) if _is_cargo(token)), None)
        if start is None:
            continue
        if "clippy" not in (token.strip("\"'`") for token in tokens[start + 1 :]):
            continue
        found.append((number, joined.strip()))
    return found


def variable_selections(text: str) -> dict[str, list[list[str]]]:
    """Every `NAME = ...` in a makefile, as {name: [values, values...]}.

    All the definitions are kept, not the first one. Make takes the last
    assignment before use, so a Makefile that sets `PACKAGES` twice -- the second
    time to one crate -- runs narrow, and a check that read only the first
    definition would call that run whole. Every definition has to name a
    workspace token for the variable to mean the workspace.
    """
    found: dict[str, list[list[str]]] = {}
    for _, joined in _logical_lines(text):
        for token_line in joined.split(";"):
            match = ASSIGNMENT.match(token_line.strip())
            if match is None:
                continue
            found.setdefault(match.group(1), []).append(match.group(2).split())
    return found


def _workspace_variables(selections: dict[str, list[list[str]]]) -> set[str]:
    """Variables that can be shown to mean the whole workspace.

    Every definition of a name has to carry a workspace token, because make takes
    the last assignment before use. A name with no definition is not in this set:
    `$(CARGO)` is defined by the environment or by the Makefile's own
    conditional machinery, and treating an unknown variable as a selector is how
    the executable itself came to satisfy rule 1.
    """
    return {
        name
        for name, values in selections.items()
        if values and all(any(token in SCOPE_TOKENS for token in value) for value in values)
    }


def _bare_shim(value: list[str]) -> bool:
    """Whether one `NAME = ...` definition names a bare shim executable.

    A definition is accepted only when it is exactly one word from
    `SHIM_EXECUTABLES`. An empty value, a path, or a value that is itself a
    variable reference is not accepted, because none of them can be shown to be
    the shim on `PATH`.
    """
    return len(value) == 1 and value[0].strip("\"'`").lower() in SHIM_EXECUTABLES


def _toolchain_problem(
    executable: str,
    line: str,
    selections: dict[str, list[list[str]]],
) -> str | None:
    """Why this clippy run does not read the pinned toolchain, or None.

    A toolchain binary invoked by path does not read `rust-toolchain.toml`, so a
    run that reaches cargo through one lints with whichever compiler that
    toolchain directory happens to hold. Once the pin moves, that is a different
    compiler from the one the runner uses, and the local gate reports green on a
    tree the runner rejects. Only a bare name reaches cargo through the shim, and
    the shim is what reads the pin.

    A `$(NAME)` or `${NAME}` reference is judged on the definitions rather than
    on its own text, which is what makes `CARGO ?= cargo` pass: the reference
    expands to a bare `cargo`, so the run does read the pin. It is refused when
    any definition of that name is a path, because make takes the last assignment
    before use and one path is enough, and when the file never defines the name,
    because then nothing here can be shown to be a bare `cargo` -- the `$(CARGO)`
    case in a workflow, where no Makefile was read at all.

    A definition naming another variable is refused rather than followed. That is
    this rule's conservative direction: one level of indirection has no
    definition here to be checked against, and this Makefile needs none.
    """
    raw = executable.strip("\"'`")
    if raw.lower() in SHIM_EXECUTABLES:
        return None
    name, _ = _unwrap_variable(executable)
    if name is None:
        because = f"a clippy run through `{raw}`, which is not a bare `cargo`. "
    elif name not in selections:
        because = (
            f"a clippy run through `{raw}`, which this file never defines. What "
            "it expands to cannot be shown to be a bare `cargo`, so a definition "
            "is required before the reference is trusted. "
        )
    elif not all(_bare_shim(value) for value in selections[name]):
        because = (
            f"a clippy run through `{raw}`, and not every definition of `{name}` "
            "in this file is a bare `cargo`. Make takes the last assignment "
            "before use, so one definition carrying a toolchain path is enough. "
        )
    else:
        return None
    return (
        f"{because}A toolchain binary invoked by path does not read "
        "`rust-toolchain.toml`, so the run lints with the compiler this host "
        f"carries rather than the one the repository pins: `{line}`"
    )


def _toolchain_pin_suffix(after: list[str], line: str) -> str | None:
    """Why the tokens after the cargo executable name the toolchain, or None.

    `cargo +nightly clippy` is rustup's own first-documented override, and
    `rustup help run` states it is the same thing as `rustup run --install
    nightly cargo`. It sits *after* the executable rather than before it, so
    `_toolchain_override` cannot see it: there is nothing in front of the cargo
    token to read. Accepted with exit 0, it lints with whichever compiler the
    name selects and never reads the toolchain file, which is the defect this
    rule exists for.

    Only the first token after the executable is read, because that is where
    rustup expects the override and a `+name` anywhere else is an argument to
    clippy rather than a toolchain. It must be a plain name: `+` alone is not a
    toolchain, and refusing it would refuse a spelling that names nothing.
    """
    if not after or not after[0].startswith("+"):
        return None
    named = after[0][1:]
    if not named or not re.fullmatch(r"[A-Za-z0-9._-]+", named):
        return None
    return (
        f"a clippy run under `cargo +{named}`, which names the toolchain on the "
        "command line. Rustup resolves a named toolchain ahead of the toolchain "
        "file, so the pin is not read and the run lints with the compiler that "
        f"name selects rather than the one the repository pins: `{line}`"
    )


def _toolchain_override(prefix: list[str], line: str) -> str | None:
    """Why the tokens before the cargo executable name the toolchain, or None.

    Only the executable itself was read before, so a line that reached cargo
    through the shim and then chose a toolchain around it passed. Two such lines
    were accepted with exit 0 while linting with a compiler the repository does
    not pin: `rustup run stable cargo clippy` and `RUSTUP_TOOLCHAIN=stable cargo
    clippy`.

    `rustup run` is refused only when it actually names a toolchain. With no name
    between `run` and cargo there is nothing to override, and on rustup 1.29.1
    `<TOOLCHAIN>` is a required argument, so that spelling is not a lint
    invocation at all. Refusing every line containing `rustup` would refuse the
    one spelling that is safe along with the one that is not.
    """
    for index, token in enumerate(prefix):
        if token != "rustup":
            continue
        rest = prefix[index + 1 :]
        if rest and rest[0].startswith("+"):
            spelling = rest[0]
        elif len(rest) > 1 and rest[0] == "run":
            spelling = f"run {rest[1]}"
        else:
            continue
        return (
            f"a clippy run under `rustup {spelling}`, which names the toolchain on "
            "the command line. Rustup resolves a named toolchain ahead of the "
            "toolchain file, so the pin is not read and the run lints with the "
            "compiler that name selects rather than the one the repository pins: "
            f"`{line}`"
        )
    for index, token in enumerate(prefix):
        match = ENV_ASSIGNMENT.match(token)
        if match is not None:
            name, value = match.group("name"), match.group("value")
        elif (
            token in TOOLCHAIN_OVERRIDES
            and len(prefix) > index + 1
            and prefix[index + 1] == "="
        ):
            # `NAME = value` splits into three tokens, so the value is the one
            # after the `=`.
            name, value = token, prefix[index + 2] if len(prefix) > index + 2 else ""
        else:
            continue
        if name not in TOOLCHAIN_OVERRIDES:
            continue
        if name == "RUSTUP_TOOLCHAIN":
            lead = (
                f"`{name}={value}` is an override rustup resolves ahead of the "
                "toolchain file, so the pin is never read"
            )
        elif name == "CARGO":
            lead = (
                f"`{name}={value}` names the cargo a subcommand is handed, which "
                "is the same toolchain choice as reaching cargo by path"
            )
        else:
            lead = (
                f"`{name}={value}` is the tool cargo was told to run instead of "
                "the one it would pick, so the toolchain file does not reach it"
            )
        return (
            f"a clippy run behind {lead}. The run then lints with the compiler "
            f"this line names rather than the one the repository pins: `{line}`"
        )
    return None


def _scope_problem(
    line: str,
    whole: set[str],
    selections: dict[str, list[list[str]]],
) -> str | None:
    """Why this command cannot be shown to select every crate, or None.

    `--exclude` is refused before anything else is looked at, because it is the
    one-word edit that restores the original defect while the line still reads as
    a workspace-wide run: cargo documents `--exclude` as usable only together
    with `--workspace`, so it always removes a crate from one.

    Everything that chooses the toolchain is refused next, and the tokens before
    the executable before the executable itself. A bare `cargo` is the shim, but a
    bare `cargo` wrapped in `rustup run <toolchain>` or handed an overriding
    environment is not, and a run with the wrong compiler is the worse defect
    whichever packages it covers.

    Only the part of the line after the cargo executable is consulted for the
    selector, so the executable written as `$(CARGO)` is never mistaken for a
    scope token.
    """
    tokens = line.split()
    start = next((i for i, token in enumerate(tokens) if _is_cargo(token)), 0)
    tail = " ".join(tokens[start + 1 :])
    if any(token == "--exclude" or token.startswith("--exclude=") for token in tokens):
        return (
            "a clippy run carrying --exclude, which drops a crate out of a "
            f"workspace run while the line still reads as one: `{line}`"
        )
    override = _toolchain_override(tokens[:start], line)
    if override is not None:
        return override
    problem = _toolchain_problem(tokens[start], line, selections)
    if problem is not None:
        return problem
    pinned = _toolchain_pin_suffix(tokens[start + 1 :], line)
    if pinned is not None:
        return pinned
    if any(token in SCOPE_TOKENS for token in tokens):
        return None
    if any(token in ("-p", "--package") or token.startswith("--package=") for token in tokens):
        return None
    # At least one name is required. `all()` over an empty list is true, which
    # let a command with no selector at all pass on the strength of a variable
    # that was never there.
    names = VARIABLE.findall(tail)
    if names and all(name in whole for name in names):
        return None
    return (
        "a clippy run naming nothing it can be shown to select. Without a "
        "selector cargo falls back to the manifest's default selection, which is "
        f"how this gate passed while linting one crate: `{line}`"
    )


def _deprecation_note(line: str) -> str | None:
    """A deprecated scope spelling on this line, or None."""
    for token in line.split():
        replacement = DEPRECATED_ALIASES.get(token)
        if replacement is not None:
            return f"`{token}` is a deprecated alias for `{replacement}`"
    return None


class Finding:
    """One thing that is wrong, and where it was found."""

    def __init__(self, path: str, line: int, rule: str, message: str) -> None:
        self.path = path
        self.line = line
        self.rule = rule
        self.message = message

    def __str__(self) -> str:
        where = f"{self.path}:{self.line}" if self.line else self.path
        return f"{where}: {self.rule}: {self.message}"


class Surface:
    """The files this check has to read, and the verdict on each."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.findings: list[Finding] = []
        self.unreadable: list[str] = []
        self.scanned: list[str] = []
        self.invocations = 0

    def exit_code(self) -> int:
        """The verdict. A path this check could not read outranks a finding.

        The unreadable case comes first on purpose: a clean run over the files it
        did reach is not a clean run, and reporting it as one is the failure this
        check was written after.
        """
        if self.unreadable:
            return EXIT_UNREADABLE
        return EXIT_FINDINGS if self.findings else EXIT_OK

    def read(self, relative: str) -> str | None:
        path = self.root / relative
        self.scanned.append(relative)
        if not path.exists():
            # A missing file is already a finding from REQUIRED. Recording it as
            # unreadable as well would report one absence twice and give the run
            # the wrong exit code.
            return None
        try:
            return path.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            self.unreadable.append(f"{relative}: {error.strerror or error}")
            return None

    def invocations_in(
        self,
        relative: str,
        text: str,
        whole: set[str],
        selections: dict[str, list[list[str]]],
    ) -> int:
        """Check every clippy run in one file, and return how many there were."""
        found = clippy_invocations(text)
        for number, line in found:
            problem = _scope_problem(line, whole, selections)
            if problem is not None:
                self.findings.append(Finding(relative, number, "scope", problem))
            note = _deprecation_note(line)
            if note is not None:
                self.findings.append(Finding(relative, number, "deprecated-scope", note))
        self.invocations += len(found)
        return len(found)


def _manifest_findings(surface: Surface) -> None:
    text = surface.read("Cargo.toml")
    if text is None:
        return
    in_workspace = False
    for number, raw in enumerate(text.splitlines(), start=1):
        if WORKSPACE_TABLE.match(raw):
            in_workspace = True
            continue
        if TABLE_HEADER.match(raw):
            in_workspace = False
            continue
        if in_workspace and DEFAULT_MEMBERS.match(raw):
            surface.findings.append(
                Finding(
                    "Cargo.toml",
                    number,
                    "default-selection",
                    "the [workspace] table narrows the default package selection, "
                    "so every unqualified cargo command means this subset rather "
                    "than the workspace",
                )
            )


def _shell_paths(root: Path) -> list[str]:
    """Every shell script in the bounded search, as a relative path."""
    paths: list[str] = []
    for directory in (".",) + SHELL_DIRS:
        try:
            found = sorted((root / directory).glob("*.sh"))
        except OSError:
            continue
        paths.extend(str(path.relative_to(root)) for path in found if path.is_file())
    paths.extend(name for name in SHELL_EXTRA if (root / name).is_file())
    return sorted(dict.fromkeys(paths))


def scan(root: Path) -> Surface:
    surface = Surface(root)

    for relative in REQUIRED:
        if not (root / relative).exists():
            surface.findings.append(
                Finding(relative, 0, "surface", "a file this check must read is missing")
            )

    makefile = surface.read("Makefile")
    whole: set[str] = set()
    selections: dict[str, list[list[str]]] = {}
    if makefile is not None:
        selections = variable_selections(makefile)
        whole = _workspace_variables(selections)
        # No finding is raised here for a variable that merely fails to name the
        # workspace. Most variables in this Makefile are paths and flags, and
        # reporting all of them produced twenty-four findings on a clean tree. A
        # narrowing variable is caught where it matters instead: on the clippy
        # line that reads it, below.
        if surface.invocations_in("Makefile", makefile, whole, selections) == 0:
            surface.findings.append(
                Finding(
                    "Makefile",
                    0,
                    "presence",
                    "no clippy run at all. A check that reports on the lines it "
                    "finds would call an empty file clean, which is how a target "
                    "ends up keeping its name after the check stops running",
                )
            )

    _manifest_findings(surface)

    try:
        workflow_dir = root / ".github/workflows"
        workflows = sorted(workflow_dir.glob("*.yml")) + sorted(workflow_dir.glob("*.yaml"))
    except OSError as error:
        surface.unreadable.append(f".github/workflows: {error.strerror or error}")
        workflows = []
    if not workflows:
        surface.unreadable.append(".github/workflows: no workflow file could be listed")

    for workflow in workflows:
        relative = f".github/workflows/{workflow.name}"
        text = surface.read(relative)
        if text is None:
            continue
        count = surface.invocations_in(relative, text, whole, selections)
        if count == 0 and "clippy" in text.lower():
            surface.findings.append(
                Finding(
                    relative,
                    0,
                    "presence",
                    "this file still mentions clippy but runs none of it",
                )
            )

    for relative in _shell_paths(root):
        text = surface.read(relative)
        if text is not None:
            surface.invocations_in(relative, text, whole, selections)

    if surface.invocations == 0:
        surface.findings.append(
            Finding(
                "",
                0,
                "presence",
                "no clippy run was found anywhere in the files this check reads",
            )
        )
    return surface


def report(root: Path) -> int:
    surface = scan(root)
    for path in surface.unreadable:
        print(f"lint-scope-check: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"lint-scope-check: FINDING {finding}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        print(
            f"lint-scope-check: {len(surface.unreadable)} path(s) this check had to read could "
            "not be read, so the rest of this run does not cover them",
            file=sys.stderr,
        )
        return code
    if code == EXIT_FINDINGS:
        print(f"lint-scope-check: {len(surface.findings)} finding(s)")
        return code
    print(
        f"lint-scope-check: every clippy run names what it selects "
        f"({surface.invocations} run(s) over {len(surface.scanned)} file(s))"
    )
    return code


# --- Self-test -----------------------------------------------------------------
#
# Every case is a fixture tree, so the cases describe the check rather than the
# repository and the suite gives the same answer on any machine. The fixtures live
# under one temporary directory that is removed when the suite returns, because a
# case per directory left sixteen trees behind on every pre-commit and every CI
# run. Nothing here is executed, so the executable-temporary-directory helper the
# other suites need is not needed: no fixture has to run.

CLEAN_MAKEFILE = """PACKAGES ?= --workspace

ritual:
\tcargo fmt --all -- --check
\tcargo clippy --workspace --all-targets --all-features --locked -- -D warnings

lint:
\tcargo clippy $(PACKAGES) --all-targets --all-features --locked -- -D warnings
"""

CLEAN_WORKFLOW = """on: [push]
jobs:
  check:
    steps:
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
      - name: Clippy
        run: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
"""

CLEAN_MANIFEST = '[workspace]\nmembers = ["a", "b"]\n\n[workspace.package]\nversion = "0.1.0"\n'

CLEAN_FILES = {
    "Makefile": CLEAN_MAKEFILE,
    "Cargo.toml": CLEAN_MANIFEST,
    ".github/workflows/ci.yml": CLEAN_WORKFLOW,
}

#: One bare clippy line, used by the cases that vary something else.
BARE = "ritual:\n\tcargo clippy --all-targets --all-features --locked -- -D warnings\n"

BARE_IN_WORKFLOW = """on: [push]
jobs:
  check:
    steps:
      - name: Clippy
        run: cargo clippy --all-targets --all-features --locked -- -D warnings
"""

#: A toolchain named on the command line in front of a bare `cargo`, and the same
#: run with no name at all. The clippy arguments are spelled out in each fixture
#: rather than produced by editing `CLEAN_MAKEFILE`, because substring surgery on
#: a fixture is how a case ends up testing a line it never built.
RUSTUP_RUN_MAKEFILE = (
    "ritual:\n\trustup run stable cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

RUSTUP_RUN_BARE_MAKEFILE = (
    "ritual:\n\trustup run cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

RUSTUP_TOOLCHAIN_MAKEFILE = (
    "ritual:\n\tRUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

RUSTC_ENV_MAKEFILE = (
    "ritual:\n\tRUSTC=/some/path/rustc cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

#: The override sits *after* the executable, so nothing in front of the cargo
#: token carries it and the prefix rules cannot see it at all.
CARGO_PLUS_TC_MAKEFILE = (
    "ritual:\n\tcargo +stable clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

#: One bare line and one overridden line, so the case cannot pass on the count
#: alone: the finding has to name the `+stable` line specifically.
CARGO_PLUS_TC_MIXED_MAKEFILE = (
    "lint:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"
    "\nritual:\n\tcargo +nightly clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

#: `+` with no name is not a toolchain, so refusing it would refuse a spelling
#: that overrides nothing.
CARGO_PLUS_BARE_MAKEFILE = (
    "ritual:\n\tcargo + clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

#: Two clippy lines, so the brace case cannot pass on there being at least one
#: invocation: the bare line is what keeps the run count up, and the `${CARGO}`
#: line is the one that has to be found and refused.
BRACE_CARGO_MAKEFILE = (
    "lint:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"
    "\nritual:\n\t${CARGO} clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

CARGO_BARE_MAKEFILE = (
    "CARGO ?= cargo\n\nritual:\n\t$(CARGO) clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

#: The same fallback written the other way round, as an export.
CARGO_EXPORT_MAKEFILE = (
    "export CARGO = cargo\n\nritual:\n\t$(CARGO) clippy --workspace"
    " --all-targets --locked -- -D warnings\n"
)

CARGO_TWICE_MAKEFILE = (
    "CARGO ?= cargo\nCARGO := /usr/bin/cargo\n\nritual:\n"
    "\t$(CARGO) clippy --workspace --all-targets --all-features --locked"
    " -- -D warnings\n"
)


def _fixture(base: Path, label: str, changes: dict[str, str | None] | None = None) -> Path:
    """One case's tree: the clean files, with `changes` applied over them.

    The overrides are a dict of real relative paths rather than keyword arguments,
    because a keyword cannot carry the dot in `Cargo.toml` and a path mangling
    scheme cannot carry the slashes. Getting that wrong wrote the override to
    `Cargo/toml`, left the clean manifest in place, and two cases passed because
    they were testing the wrong file.
    """
    files = dict(CLEAN_FILES)
    for relative, body in (changes or {}).items():
        if body is None:
            files.pop(relative, None)
        else:
            files[relative] = body
    root = base / label
    for relative, text in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return root


def _cases(base: Path) -> list[tuple[str, int, Path]]:
    """(name, expected exit, fixture root) for every case needing no refusal."""
    wf = ".github/workflows/ci.yml"
    return [
        ("a clean tree passes", EXIT_OK, _fixture(base, "clean")),
        ("a bare clippy in the Makefile is a finding", EXIT_FINDINGS, _fixture(base, "makefile-bare", {"Makefile": BARE})),
        ("a bare clippy in a workflow is a finding", EXIT_FINDINGS, _fixture(base, "workflow-bare", {wf: BARE_IN_WORKFLOW})),
        ("a bare clippy in a shell script is a finding", EXIT_FINDINGS, _fixture(base, "script-bare", {"scripts/lint.sh": "#!/bin/sh\ncargo clippy --all-targets -- -D warnings\n"})),
        (
            "a shell script holding a scoped clippy is clean, at the same path",
            EXIT_OK,
            _fixture(base, "script-clean", {"scripts/lint.sh": "#!/bin/sh\ncargo clippy --workspace --all-targets -- -D warnings\n"}),
        ),
        (
            "the shim carrying a dispatch arm is read and stays clean",
            EXIT_OK,
            _fixture(base, "shim", {"scripts/gate-shim": 'case "$1" in\n  test | nextest | llvm-cov | clippy) heavy=1 ;;\nesac\n'}),
        ),
        ("--all-targets alone is not a package selector", EXIT_FINDINGS, _fixture(base, "all-targets", {"Makefile": "ritual:\n\tcargo clippy --all-targets --locked -- -D warnings\n"})),
        (
            "--workspace --exclude drops a crate and is a finding",
            EXIT_FINDINGS,
            _fixture(base, "exclude-makefile", {"Makefile": "ritual:\n\tcargo clippy --workspace --exclude clio-store --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "--workspace --exclude in a workflow is a finding",
            EXIT_FINDINGS,
            _fixture(base, "exclude-workflow", {wf: CLEAN_WORKFLOW.replace("--workspace --all-targets", "--workspace --exclude clio-store --all-targets")}),
        ),
        # The executable, which is a different defect from the scope of the run.
        # The `$(CARGO)` form is the line this rule was added for, written out
        # rather than produced by editing the clean fixture, because substring
        # surgery on a fixture is how a case ends up testing a line it never built.
        (
            "$(CARGO) clippy is a finding",
            EXIT_FINDINGS,
            _fixture(base, "cargo-var", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\t$(CARGO) clippy --workspace --all-targets --all-features --locked -- -D warnings\n"}),
        ),
        (
            "$(RUSTUP_BIN)/cargo clippy is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustup-bin", {"Makefile": "ritual:\n\t$(RUSTUP_BIN)/cargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a clippy run through an absolute path is a finding",
            EXIT_FINDINGS,
            _fixture(base, "abs-path", {"Makefile": "ritual:\n\t/usr/local/rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a bare cargo clippy in the Makefile is clean",
            EXIT_OK,
            _fixture(base, "makefile-shim", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "$(CARGO) on a line that runs no clippy is clean, and the workflow's bare cargo with it",
            EXIT_OK,
            _fixture(base, "cargo-var-no-clippy", {"Makefile": "CARGO ?= /usr/bin/cargo\nPACKAGES ?= --workspace\n\nbuild:\n\t$(CARGO) build --locked\n\nritual:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a trailing comment naming the selector does not supply it",
            EXIT_FINDINGS,
            _fixture(base, "todo-comment", {"Makefile": BARE.rstrip("\n") + " # TODO: add --workspace\n"}),
        ),
        (
            "a trailing comment in a workflow does not supply the selector",
            EXIT_FINDINGS,
            _fixture(base, "todo-workflow", {wf: BARE_IN_WORKFLOW.rstrip("\n") + "\n          # same as --workspace\n"}),
        ),
        (
            "a second PACKAGES definition that narrows is a finding",
            EXIT_FINDINGS,
            _fixture(base, "packages-twice", {"Makefile": CLEAN_MAKEFILE.replace("PACKAGES ?= --workspace", "PACKAGES ?= --workspace\nPACKAGES := -p clio")}),
        ),
        (
            "a wrapped recipe is read as one command",
            EXIT_OK,
            _fixture(base, "wrapped", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\tcargo clippy \\\n\t\t--workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a selector in any variable, not only PACKAGES, is honoured",
            EXIT_OK,
            _fixture(base, "other-variable", {"Makefile": "SCOPE := --workspace\n\nritual:\n\tcargo clippy $(SCOPE) --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a variable that does not name the workspace is a finding",
            EXIT_FINDINGS,
            _fixture(base, "other-variable-narrow", {"Makefile": "SCOPE := -p clio\n\nritual:\n\tcargo clippy $(SCOPE) --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "-p names packages and passes",
            EXIT_OK,
            _fixture(base, "dash-p", {"Makefile": "ritual:\n\tcargo clippy -p a -p b --all-targets --locked -- -D warnings\n"}),
        ),
        ("$(PACKAGES) is accepted while it still means the workspace", EXIT_OK, _fixture(base, "packages-whole")),
        (
            "$(PACKAGES) narrowed to one crate is a finding",
            EXIT_FINDINGS,
            _fixture(base, "packages-narrow", {"Makefile": CLEAN_MAKEFILE.replace("PACKAGES ?= --workspace", "PACKAGES ?= -p clio")}),
        ),
        (
            "default-members is a finding",
            EXIT_FINDINGS,
            _fixture(base, "default-members", {"Cargo.toml": '[workspace]\nmembers = ["a"]\ndefault-members = ["a"]\n'}),
        ),
        (
            "a quoted default-members key is the same key",
            EXIT_FINDINGS,
            _fixture(base, "default-members-quoted", {"Cargo.toml": '[workspace]\nmembers = ["a"]\n"default-members" = ["a"]\n'}),
        ),
        (
            "default-members outside [workspace] is not a finding",
            EXIT_OK,
            _fixture(base, "default-members-elsewhere", {"Cargo.toml": '[workspace]\nmembers = ["a"]\n\n[profile.dev]\ndefault-members = ["a"]\n'}),
        ),
        ("a deprecated --all is reported and still names the scope", EXIT_FINDINGS, _fixture(base, "all-alias", {"Makefile": CLEAN_MAKEFILE.replace("clippy --workspace", "clippy --all")})),
        ("a dispatch arm naming clippy is not an invocation", EXIT_OK, _fixture(base, "dispatch-arm", {"hooks.d/x.sh": 'case "$1" in\n  test | clippy) heavy=1 ;;\nesac\n'})),
        ("a commented clippy line is not an invocation", EXIT_OK, _fixture(base, "commented", {"Makefile": CLEAN_MAKEFILE + "# $(CARGO) clippy --all-targets -- -D warnings\n"})),
        ("a missing Makefile is a finding, not a pass", EXIT_FINDINGS, _fixture(base, "no-makefile", {"Makefile": None})),
        (
            "a Makefile with no clippy run is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-clippy", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\tcargo fmt --all -- --check\n", wf: "on: [push]\njobs: {}\n"}),
        ),
        (
            "a workflow that mentions clippy but runs none is a finding",
            EXIT_FINDINGS,
            _fixture(base, "workflow-lost", {wf: "on: [push]\njobs:\n  check:\n    steps:\n      - name: Clippy would go here\n"}),
        ),
        # A toolchain named around a bare `cargo`, which the executable check
        # cannot see because the executable really is the bare shim. Each of
        # these was accepted with exit 0 before.
        (
            "rustup run naming a toolchain before a bare cargo is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustup-run", {"Makefile": RUSTUP_RUN_MAKEFILE}),
        ),
        (
            "rustup run naming no toolchain before a bare cargo is clean",
            EXIT_OK,
            _fixture(base, "rustup-run-bare", {"Makefile": RUSTUP_RUN_BARE_MAKEFILE}),
        ),
        (
            "RUSTUP_TOOLCHAIN on the clippy line is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustup-toolchain-env", {"Makefile": RUSTUP_TOOLCHAIN_MAKEFILE}),
        ),
        (
            "cargo +toolchain before the subcommand is a finding",
            EXIT_FINDINGS,
            _fixture(base, "cargo-plus-tc", {"Makefile": CARGO_PLUS_TC_MAKEFILE}),
        ),
        (
            "cargo +toolchain is refused while a bare cargo on another line is clean",
            EXIT_FINDINGS,
            _fixture(base, "cargo-plus-tc-mixed", {"Makefile": CARGO_PLUS_TC_MIXED_MAKEFILE}),
        ),
        (
            "a bare + with no toolchain name is not a toolchain override",
            EXIT_OK,
            _fixture(base, "cargo-plus-bare", {"Makefile": CARGO_PLUS_BARE_MAKEFILE}),
        ),
        (
            "RUSTC on the clippy line is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustc-env", {"Makefile": RUSTC_ENV_MAKEFILE}),
        ),
        (
            "a second CARGO definition carrying a path is a finding",
            EXIT_FINDINGS,
            _fixture(base, "cargo-path-twice", {"Makefile": CARGO_TWICE_MAKEFILE}),
        ),
        # The executable, resolved through the file's own definitions.
        (
            "a ${CARGO} clippy line is a finding",
            EXIT_FINDINGS,
            _fixture(base, "brace-cargo", {"Makefile": BRACE_CARGO_MAKEFILE}),
        ),
        (
            "$(CARGO) holding a bare cargo is clean",
            EXIT_OK,
            _fixture(base, "cargo-bare", {"Makefile": CARGO_BARE_MAKEFILE}),
        ),
        (
            "an exported CARGO holding a bare cargo is clean",
            EXIT_OK,
            _fixture(base, "cargo-export", {"Makefile": CARGO_EXPORT_MAKEFILE}),
        ),
    ]


def _refusal_case(base: Path) -> Path | None:
    """A fixture whose only problem is a file this process cannot read.

    None when the refusal cannot be provoked here. A host that reads a mode-000
    file anyway reports the skip, because a case that cannot be set up is not a
    case that ran.
    """
    root = _fixture(base, "unreadable", {"scripts/lint.sh": "#!/bin/sh\ncargo clippy --workspace\n"})
    blocked = root / "scripts" / "lint.sh"
    blocked.chmod(0o000)
    try:
        blocked.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return root
    return None


def _self_test() -> int:
    failures = 0
    total = 0
    skipped = 0
    with tempfile.TemporaryDirectory(prefix="lint-scope-") as scratch:
        base = Path(scratch)
        for name, want, root in _cases(base):
            total += 1
            got = scan(root).exit_code()
            ok = got == want
            print(f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want})")
            failures += 0 if ok else 1

        # The cases above read an exit code, and an exit code cannot say which
        # rule fired or on which line. A fixture whose Makefile was never read
        # raises the same code as one whose clippy line was refused, which is
        # exactly how a case passes for the wrong reason. So this asserts the
        # finding itself, against a whole fixture tree: the Makefile carries the
        # executable by reference, the workflow carries a bare one, and the rule
        # has to name the first and leave the second alone.
        total += 1
        mixed = _fixture(
            base,
            "executable-content",
            {
                "Makefile": "PACKAGES ?= --workspace\n\nritual:\n"
                "\t$(CARGO) clippy --workspace --all-targets --locked -- -D warnings\n"
            },
        )
        surface = scan(mixed)
        # Matched on the reason the message states, not on a rule label, because
        # the label is the check's business and the reason is the finding's.
        refused = [f for f in surface.findings if "rust-toolchain.toml" in f.message]
        want_line = next(
            number
            for number, raw in enumerate((mixed / "Makefile").read_text().splitlines(), start=1)
            if "$(CARGO) clippy" in raw
        )
        got_where = [(f.path, f.line) for f in refused]
        ok = (
            surface.exit_code() == EXIT_FINDINGS
            and got_where == [("Makefile", want_line)]
            and "`$(CARGO)`" in refused[0].message
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the finding names the Makefile line that "
            f"reaches cargo through $(CARGO) and not the workflow's bare cargo "
            f"(reported {got_where}, want [('Makefile', {want_line})])"
        )
        failures += 0 if ok else 1

        # An exit code cannot say which rule fired, on which line, or what the
        # check printed. The three assertions below read the findings and the
        # output, so a case that passed because a different rule fired, or
        # because the fixture's own Makefile line was never read, cannot pass.
        total += 1
        brace = _fixture(base, "brace-counted", {"Makefile": BRACE_CARGO_MAKEFILE})
        brace_surface = scan(brace)
        brace_lines = clippy_invocations((brace / "Makefile").read_text(encoding="utf-8"))
        brace_findings = [f for f in brace_surface.findings if f.path == "Makefile"]
        ok = (
            len(brace_lines) == 2
            and len(brace_findings) == 1
            and brace_findings[0].line == 5
            and "${CARGO}" in brace_findings[0].message
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: both clippy lines are counted and only the "
            f"${{CARGO}} one is refused (counted {len(brace_lines)}, want 2; findings on lines "
            f"{[f.line for f in brace_findings]}, want [5])"
        )
        failures += 0 if ok else 1

        total += 1
        override = _fixture(base, "override-printed", {"Makefile": RUSTUP_TOOLCHAIN_MAKEFILE})
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            got = report(override)
        output = printed.getvalue()
        ok = (
            got == EXIT_FINDINGS
            and "lint-scope-check: FINDING Makefile:2: scope:" in output
            and "`RUSTUP_TOOLCHAIN=stable` is an override rustup resolves ahead of the "
            "toolchain file" in output
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the whole check reports the override line with "
            f"its file, line, rule and reason (exit {got}, want {EXIT_FINDINGS})"
        )
        failures += 0 if ok else 1

        total += 1
        bare = _fixture(base, "bare-definitions", {"Makefile": CARGO_BARE_MAKEFILE})
        bare_surface = scan(bare)
        ok = (
            bare_surface.exit_code() == EXIT_OK
            and not [f for f in bare_surface.findings if f.path == "Makefile"]
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: $(CARGO) holding a bare cargo is clean under the "
            f"whole check (exit {bare_surface.exit_code()}, want {EXIT_OK}, "
            f"findings {[str(f) for f in bare_surface.findings]})"
        )
        failures += 0 if ok else 1

        root = _refusal_case(base)
        if root is None:
            total += 1
            skipped += 1
            print("self-test skipped: this process reads a mode-000 file, so the refusal cannot be provoked here")
        else:
            total += 1
            got = scan(root).exit_code()
            ok = got == EXIT_UNREADABLE
            print(
                f"self-test {'ok' if ok else 'FAIL'}: a clippy script this process cannot read "
                f"fails closed (exit {got}, want {EXIT_UNREADABLE})"
            )
            failures += 0 if ok else 1
            (root / "scripts" / "lint.sh").chmod(0o600)

    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total - skipped} cases, {skipped} skipped)")
    return EXIT_OK


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return EXIT_OK
    root = Path(__file__).resolve().parent.parent
    if "--list-scanned" in argv:
        for relative in scan(root).scanned:
            print(relative)
        return EXIT_OK
    if argv:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    return report(root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
