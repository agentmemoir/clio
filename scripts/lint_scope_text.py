"""Reading Makefile, workflow and script text for clippy lines, and the variables that select a scope."""

from __future__ import annotations

import re

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
