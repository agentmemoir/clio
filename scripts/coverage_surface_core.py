"""The scan over the coverage surface, its findings, and the report."""

from __future__ import annotations

import sys
from pathlib import Path

from coverage_surface_invocations import (
    Invocation,
    _cargo_index,
    coverage_invocations,
    names_subset,
    runs_tests,
    writes_report,
)
from coverage_surface_runner import (
    _boundary_problem,
    _hygiene_problem,
    _object_problem,
    _report_problem,
    builder_body,
    strip_python_prose,
)
from coverage_surface_text import (
    MAKEFILE,
    _tokens,
    comment_kind,
    strip_comments,
    variable_selections,
)

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

#: The gate itself. Its two command builders are where the boundary this check
#: protects is written, and its hygiene steps are where the boundary is used.
#: The builders and the whole-workspace run are in `scoped_run.py`; the one-package
#: run is in `scoped_package.py`, which defines no builder and is read for rule 4.
RUNNER = "scripts/scoped_run.py"
PACKAGE_RUNNER = "scripts/scoped_package.py"

#: Files that must exist for this check to have read the whole surface. A missing
#: one is a finding rather than a pass, for the reason rule 4 gives.
REQUIRED = (RUNNER, PACKAGE_RUNNER, MAKEFILE)

#: The two command builders. Named rather than grepped, so a Python file's prose
#: and its own fixtures cannot read as coverage commands.
TEST_BUILDER = "coverage_test_command"
REPORT_BUILDER = "coverage_report_command"

#: The mark that says a file restricts the objects its report may merge, and the
#: key that says it pins an instrumented directory of its own. Either one is
#: enough, and both are here because they are the two ways this repository has
#: answered the foreign-object problem.
PRUNE_MARKER = "prune_foreign_objects("
DIRECTORY_KEY = "CARGO_LLVM_COV_TARGET_DIR"

#: Directories searched for shell scripts, each non-recursively, and the shell
#: files that carry no extension. The same bounded list the lint scope check
#: uses: a walk from the repository root would descend into the build directory,
#: where there are thousands of files and none of them are commands.
SHELL_DIRS = ("scripts", "hooks.d")

SHELL_EXTRA = (
    "scripts/gate-shim",
    "scripts/git-hooks/pre-commit",
    "scripts/git-hooks/pre-merge-commit",
)

#: Why a file was refused, as the sentence a reader gets with the path.
REFUSALS = {
    "kind": "this reader has no comment model for a file of this kind, and "
    "reading it as one of the kinds it does model could cut a command in half",
    "heredoc": "this file holds a here-document, whose body is data rather than "
    "commands, so a `#` in it starts no comment and a command-shaped line in it "
    "runs nowhere",
}


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

    def code(self, relative: str, text: str) -> str | None:
        """`text` with its comments removed, or None when it is refused.

        A refusal is recorded against the unreadable paths rather than passed over,
        for the reason rule 5 gives: this reader either knows a file's comments or
        it does not judge the file, and either way it must not report a clean run
        over the part it skipped.
        """
        kind = comment_kind(relative, text)
        if kind is None:
            self.unreadable.append(f"{relative}: {REFUSALS['kind']}")
            return None
        stripped = strip_comments(text, kind)
        if stripped is None:
            self.unreadable.append(f"{relative}: {REFUSALS['heredoc']}")
            return None
        return stripped

    def check(self, relative: str, code: str) -> None:
        """Apply every rule to one file's coverage invocations.

        `code` is the file with its comments removed, so a marker found in it is
        on a line that does something. The markers are still read from the whole
        file rather than from the run, and the limit that costs is stated in the
        module docstring: a file that pins a directory for one run and leaves
        another unprotected passes here.
        """
        protected = PRUNE_MARKER in code or DIRECTORY_KEY in code
        for invocation in coverage_invocations(code, variable_selections(code)):
            self.invocations += 1
            tokens = invocation.tokens
            subset = names_subset(tokens)
            if subset and not protected:
                self.findings.append(
                    Finding(
                        relative,
                        invocation.line,
                        "object-restriction",
                        _object_problem(invocation),
                    )
                )
            if subset and runs_tests(tokens) and writes_report(tokens):
                self.findings.append(
                    Finding(relative, invocation.line, "boundary", _boundary_problem(invocation))
                )
            problem = _report_problem(invocation)
            if writes_report(tokens) and problem is not None:
                self.findings.append(
                    Finding(relative, invocation.line, "failure-mode", problem)
                )

    def check_runner(self, path: str, text: str, builders: bool = True) -> None:
        """Read the gate's two command builders, which are Python literals.

        The whole-file rules cannot see them: a Python list literal has one token
        per line and no shell to fold, so a scan that ignored the language would
        find no coverage command here at all and this check would protect nothing
        that matters.

        The markers are read from the file with its comments and its docstrings
        removed, because a note explaining that the object deletion is gone is not
        the object deletion, and because the file that holds the command builders
        is the file most likely to be edited to explain a change to them. A file
        the tokenizer refuses is refused rather than judged, for the reason
        `strip_python_prose` gives. Rule 4 reads the same stripped text, and needs
        to: a trailing comment naming a hygiene step used to satisfy it, so a run
        that had deleted both steps still read as protected as long as someone
        left the names behind in a comment. The strip blanks prose rather than
        deleting it, so a line number this check reports is the line number in the
        source.

        `builders` is False for a file that runs the steps and defines no builder.
        """
        code = strip_python_prose(text)
        if code is None:
            self.unreadable.append(
                f"{path}: this reader cannot tell its comments from its code, so a "
                "comment naming a protection would answer for one"
            )
            return
        protected = PRUNE_MARKER in code or DIRECTORY_KEY in code
        for line, rule, message in _hygiene_problem(code):
            self.findings.append(Finding(path, line, rule, message))
        for name, is_test in ((TEST_BUILDER, True), (REPORT_BUILDER, False)) if builders else ():
            found = builder_body(text, name)
            if found is None:
                self.findings.append(
                    Finding(
                        path,
                        0,
                        "boundary",
                        f"`{name}` is not defined here, so the coverage run has no "
                        f"{'test step that leaves its buffers on disk' if is_test else 'separate report step'} "
                        "to own a boundary between. One command cannot both run the "
                        "tests and write the report, because the gate has to drop a "
                        "bad buffer and a foreign object in the gap.",
                    )
                )
                continue
            number, body = found
            tokens = _tokens(body)
            start = _cargo_index(tokens)
            if start is None or "llvm-cov" not in tokens[start + 1 :]:
                continue
            invocation = Invocation(path, number, tokens[start:])
            self.invocations += 1
            subset = names_subset(tokens)
            if subset and not protected:
                self.findings.append(
                    Finding(path, number, "object-restriction", _object_problem(invocation))
                )
            if subset and runs_tests(tokens) and writes_report(tokens):
                self.findings.append(
                    Finding(path, number, "boundary", _boundary_problem(invocation))
                )
            problem = _report_problem(invocation)
            if writes_report(tokens) and problem is not None:
                self.findings.append(Finding(path, number, "failure-mode", problem))


# --- scanning -----------------------------------------------------------------


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

    for path, builders in ((RUNNER, True), (PACKAGE_RUNNER, False)):
        runner = surface.read(path)
        if runner is not None:
            surface.check_runner(path, runner, builders)

    # The Makefile includes `mk/*.mk`, which hold the coverage recipes now, so each is read like it.
    try:
        mk_files = [f"mk/{path.name}" for path in sorted((root / "mk").glob("*.mk"))]
    except OSError as error:
        surface.unreadable.append(f"mk: {error.strerror or error}")
        mk_files = []
    for relative in (MAKEFILE, *mk_files):
        makefile = surface.read(relative)
        if makefile is not None:
            stripped = surface.code(relative, makefile)
            if stripped is not None:
                surface.check(relative, stripped)

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
        if text is not None:
            stripped = surface.code(relative, text)
            if stripped is not None:
                surface.check(relative, stripped)

    for relative in _shell_paths(root):
        text = surface.read(relative)
        if text is not None:
            stripped = surface.code(relative, text)
            if stripped is not None:
                surface.check(relative, stripped)

    return surface


def report(root: Path) -> int:
    surface = scan(root)
    for path in surface.unreadable:
        print(f"coverage-surface-check: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"coverage-surface-check: FINDING {finding}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        print(
            f"coverage-surface-check: {len(surface.unreadable)} path(s) this check had to read "
            "could not be read, so the rest of this run does not cover them",
            file=sys.stderr,
        )
        return code
    if code == EXIT_FINDINGS:
        print(f"coverage-surface-check: {len(surface.findings)} finding(s)")
        return code
    print(
        f"coverage-surface-check: every coverage invocation measures one run and reports "
        f"it once ({surface.invocations} invocation(s) over {len(surface.scanned)} file(s))"
    )
    return code
