"""The scan: what each clippy run selects, the findings, and the report."""

from __future__ import annotations

import re
import sys
from pathlib import Path

from lint_scope_text import (
    DEPRECATED_ALIASES,
    SCOPE_TOKENS,
    VARIABLE,
    _is_cargo,
    _workspace_variables,
    clippy_invocations,
    variable_selections,
)
from lint_scope_toolchain import _toolchain_override, _toolchain_pin_suffix, _toolchain_problem

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

#: The root manifest, where `default-members` is rule 2.
MANIFEST = "Cargo.toml"

#: Files that must exist for this check to have read the whole surface. A missing
#: one is a finding rather than a pass, because a Makefile that had been renamed
#: would otherwise turn this check into a check of nothing.
REQUIRED = (MANIFEST, "Makefile")

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

#: TOML accepts a quoted key, so `"default-members" = [...]` is the same key.
DEFAULT_MEMBERS = re.compile(r'^\s*"?default-members"?\s*=')

WORKSPACE_TABLE = re.compile(r"^\s*\[workspace\]\s*$")

TABLE_HEADER = re.compile(r"^\s*\[")


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
