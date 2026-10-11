"""The coverage-surface self-test: each case builds a tree and states what the scan must say."""

from __future__ import annotations

import contextlib
import io
import tempfile
from pathlib import Path

from coverage_surface_core import (
    EXIT_FINDINGS,
    EXIT_OK,
    EXIT_UNREADABLE,
    REPORT_BUILDER,
    RUNNER,
    report,
    scan,
)
from coverage_surface_fixtures import (
    ANY_MODE_RUNNER,
    CLEAN_MAKEFILE,
    CLEAN_RUNNER,
    CLEAN_WORKFLOW,
    COMMENTED_AND_CALLED_RUNNER,
    COMMENTED_MARKER_MAKEFILE,
    COMMENTED_MARKER_SCRIPT,
    COMMENTED_MARKER_WORKFLOW,
    COMMENTED_OBJECT_DELETE_RUNNER,
    EXPANSION_HASH_MAKEFILE,
    HEREDOC_MAKEFILE,
    HYGIENE_AFTER_REPORT_RUNNER,
    NO_BUFFER_DROP_RUNNER,
    NO_MODE_RUNNER,
    NO_OBJECT_DELETE_RUNNER,
    NO_RUN_RUNNER,
    NO_SUBCOMMAND_BOUNDARY,
    ONE_BUILDER_RUNNER,
    ONE_COMMAND_RUNNER,
    PINNED_MAKEFILE,
    PINNED_WITH_COMMENT_MAKEFILE,
    PROSE_RUNNER,
    QUOTED_HASH_MAKEFILE,
    SPACED_MODE_RUNNER,
    SUBSET_REPORT_MAKEFILE,
    UNPROTECTED_MAKEFILE,
    WORKSPACE_AND_PACKAGE,
    WORKSPACE_ONE_COMMAND,
    _fixture,
    _without_report_builder,
)
from coverage_surface_text import MAKEFILE


def _cases(base: Path) -> list[tuple[str, int, Path, list[str]]]:
    """(name, expected exit, fixture root, rules it must prove) per case.

    The rules are named because an exit code cannot say which rule fired. A case
    that proved its rule only by producing a non-zero exit could pass with a
    different rule firing instead, which is how a suite ends up green with the
    thing it protects deleted. A list rather than a set, so a case can assert that
    one rule fired twice. An empty list means the case expects no finding at all,
    which is the assertion a clean case needs.
    """
    wf = ".github/workflows/ci.yml"
    return [
        ("a clean tree passes", EXIT_OK, _fixture(base, "clean"), []),
        (
            "a docstring naming the report subcommand is prose, not the command",
            EXIT_OK,
            _fixture(base, "prose", {RUNNER: PROSE_RUNNER}),
            [],
        ),
        # Rule 3, the failure mode.
        (
            "a report step with no failure mode is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-mode", {RUNNER: NO_MODE_RUNNER}),
            ["failure-mode"],
        ),
        (
            "a report step asking for the default failure mode is a finding",
            EXIT_FINDINGS,
            _fixture(base, "any-mode", {RUNNER: ANY_MODE_RUNNER}),
            ["failure-mode"],
        ),
        (
            "the failure mode in its other spelling is accepted",
            EXIT_OK,
            _fixture(base, "spaced-mode", {RUNNER: SPACED_MODE_RUNNER}),
            [],
        ),
        (
            "a whole-workspace report step with no failure mode is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "makefile-no-mode",
                {MAKEFILE: CLEAN_MAKEFILE.replace(" --failure-mode=all", "")},
            ),
            ["failure-mode"],
        ),
        # Rule 2, the boundary. Each of these carries a failure mode, so rule 3
        # has nothing to say about them and the exit code cannot be reached by
        # another rule: the case proves the boundary or it proves nothing.
        (
            "a test step carrying --no-run is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-run", {RUNNER: NO_RUN_RUNNER}),
            ["boundary"],
        ),
        (
            "a test step writing the report itself is a finding",
            EXIT_FINDINGS,
            _fixture(base, "one-command", {RUNNER: ONE_COMMAND_RUNNER}),
            ["boundary"],
        ),
        (
            "one command in place of the two steps is a finding, twice over",
            EXIT_FINDINGS,
            _fixture(base, "one-builder", {RUNNER: ONE_BUILDER_RUNNER}),
            ["boundary", "boundary"],
        ),
        (
            "a whole-workspace run doing both in one command is exempt, and said so",
            EXIT_OK,
            _fixture(base, "workspace-one-command", {MAKEFILE: WORKSPACE_ONE_COMMAND}),
            [],
        ),
        # Rule 1, the object list.
        (
            "a per-package command nothing protects is a finding",
            EXIT_FINDINGS,
            _fixture(base, "unprotected", {MAKEFILE: UNPROTECTED_MAKEFILE}),
            ["object-restriction"],
        ),
        (
            "a per-package run in a makefile that pins its own directory is clean",
            EXIT_OK,
            _fixture(base, "pinned", {MAKEFILE: PINNED_MAKEFILE}),
            [],
        ),
        (
            "a per-package report step runs no tests, so it is not a boundary finding",
            EXIT_OK,
            _fixture(base, "subset-report", {MAKEFILE: SUBSET_REPORT_MAKEFILE}),
            [],
        ),
        (
            "a line naming --workspace beside a package selector is not a subset",
            EXIT_OK,
            _fixture(base, "workspace-and-package", {MAKEFILE: WORKSPACE_AND_PACKAGE}),
            [],
        ),
        (
            "a per-package command naming no subcommand still runs tests, by default",
            EXIT_FINDINGS,
            _fixture(base, "no-subcommand", {MAKEFILE: NO_SUBCOMMAND_BOUNDARY}),
            ["boundary"],
        ),
        (
            "a per-package command in a shell script is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "script-narrow",
                {"scripts/one.sh": "#!/bin/sh\ncargo llvm-cov nextest -p clio --locked\n"},
            ),
            ["object-restriction"],
        ),
        (
            "a whole-workspace command in a shell script is clean",
            EXIT_OK,
            _fixture(
                base,
                "script-whole",
                {
                    "scripts/one.sh": "#!/bin/sh\n"
                    "cargo llvm-cov nextest --workspace --locked --no-report\n"
                    "cargo llvm-cov report --json --failure-mode=all\n"
                },
            ),
            [],
        ),
        (
            "a per-package command in a workflow is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "workflow-narrow",
                {wf: CLEAN_WORKFLOW + "      - name: One crate\n        run: cargo llvm-cov nextest -p clio-store --locked\n"},
            ),
            ["object-restriction"],
        ),
        (
            "the shim carrying a dispatch arm is read and stays clean",
            EXIT_OK,
            _fixture(
                base,
                "shim",
                {
                    "scripts/gate-shim": "#!/bin/sh\n"
                    'case "$1" in\n  test | nextest | llvm-cov | clippy) heavy=1 ;;\nesac\n'
                },
            ),
            [],
        ),
        (
            "a commented coverage line is not an invocation",
            EXIT_OK,
            _fixture(base, "commented", {MAKEFILE: CLEAN_MAKEFILE + "# cargo llvm-cov nextest -p clio-store\n"}),
            [],
        ),
        # Rule 1 and prose. Each of these is the shape of change a person makes
        # while removing a protection: the sentence explaining it stays and names
        # the variable it removed, so a reader that counted words rather than
        # lines reported a clean run over a run that had lost one.
        (
            "a marker in a makefile comment does not protect a per-package run",
            EXIT_FINDINGS,
            _fixture(base, "commented-marker", {MAKEFILE: COMMENTED_MARKER_MAKEFILE}),
            # Twice, because the variable and the recipe that expands it are two
            # lines carrying one command. That is this check's existing reading and
            # the count is asserted so a case cannot pass with one run found twice
            # or a second run found at all.
            ["object-restriction", "object-restriction"],
        ),
        (
            "a marker in a shell script comment does not protect a per-package run",
            EXIT_FINDINGS,
            _fixture(base, "commented-marker-script", {"scripts/one.sh": COMMENTED_MARKER_SCRIPT}),
            ["object-restriction"],
        ),
        (
            "a marker in a workflow comment does not protect a per-package run",
            EXIT_FINDINGS,
            _fixture(base, "commented-marker-workflow", {wf: COMMENTED_MARKER_WORKFLOW}),
            ["object-restriction"],
        ),
        (
            "a marker a real line pins still protects, with a comment on that line",
            EXIT_OK,
            _fixture(base, "pinned-commented", {MAKEFILE: PINNED_WITH_COMMENT_MAKEFILE}),
            [],
        ),
        (
            "a # inside an expansion is not a comment and cuts no command off",
            EXIT_FINDINGS,
            _fixture(base, "expansion-hash", {MAKEFILE: EXPANSION_HASH_MAKEFILE}),
            ["object-restriction"],
        ),
        (
            "a # inside quotes is not a comment and cuts no command off",
            EXIT_FINDINGS,
            _fixture(base, "quoted-hash", {MAKEFILE: QUOTED_HASH_MAKEFILE}),
            ["object-restriction"],
        ),
        # Rule 5 again, for a file this reader cannot strip.
        (
            "a makefile holding a here-document is refused, not judged",
            EXIT_UNREADABLE,
            _fixture(base, "heredoc", {MAKEFILE: HEREDOC_MAKEFILE}),
            [],
        ),
        # Rule 4, the hygiene steps. Each of these leaves rule 1 satisfied, because
        # the file still pins a directory of its own for its other mode, which is
        # the whole reason this rule cannot be folded into rule 1.
        (
            "a run that never deletes the objects it did not build is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-object-delete", {RUNNER: NO_OBJECT_DELETE_RUNNER}),
            ["hygiene"],
        ),
        (
            "a run that never drops an unreadable profile buffer is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-buffer-drop", {RUNNER: NO_BUFFER_DROP_RUNNER}),
            ["hygiene"],
        ),
        (
            "a step named only in a trailing comment is a finding",
            EXIT_FINDINGS,
            _fixture(
                base, "commented-object-delete", {RUNNER: COMMENTED_OBJECT_DELETE_RUNNER}
            ),
            ["hygiene"],
        ),
        (
            "a real call carrying a trailing comment is not a finding",
            EXIT_OK,
            _fixture(base, "commented-and-called", {RUNNER: COMMENTED_AND_CALLED_RUNNER}),
            [],
        ),
        (
            "a run that writes its report before cleaning is a finding",
            EXIT_FINDINGS,
            _fixture(base, "hygiene-after", {RUNNER: HYGIENE_AFTER_REPORT_RUNNER}),
            ["hygiene", "hygiene"],
        ),
        # Rule 5, and the surface a rename would empty.
        (
            "a missing runner is a finding, not a pass",
            EXIT_FINDINGS,
            _fixture(base, "no-runner", {RUNNER: None}),
            ["surface"],
        ),
        (
            "a gate with no report step at all is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "no-report-builder",
                {RUNNER: _without_report_builder(CLEAN_RUNNER)},
            ),
            ["boundary"],
        ),
    ]


def _refusal_case(base: Path) -> Path | None:
    """A fixture whose only problem is a file this process cannot read.

    None when the refusal cannot be provoked here. A host that reads a mode-000
    file anyway reports the skip, because a case that cannot be set up is not a
    case that ran.
    """
    root = _fixture(base, "unreadable")
    blocked = root / MAKEFILE
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
    with tempfile.TemporaryDirectory(prefix="coverage-surface-") as scratch:
        base = Path(scratch)
        for name, want, root, rules in _cases(base):
            total += 1
            found = scan(root)
            got = found.exit_code()
            fired = sorted(f.rule for f in found.findings)
            ok = got == want and fired == sorted(rules)
            print(
                f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want}; "
                f"rules {fired}, want {sorted(rules)})"
            )
            failures += 0 if ok else 1

        # The cases above read an exit code, and an exit code cannot say which
        # rule fired, or on which line, or what the check printed. The three
        # assertions below read the findings and the output, so a case cannot pass
        # because a different rule fired, because the fixture's own line was never
        # read, or because the finding said nothing.
        total += 1
        mixed = _fixture(base, "content", {RUNNER: NO_MODE_RUNNER})
        surface = scan(mixed)
        want_line = next(
            number
            for number, raw in enumerate((mixed / RUNNER).read_text().splitlines(), start=1)
            if raw.startswith(f"def {REPORT_BUILDER}")
        )
        got_where = [(f.path, f.line, f.rule) for f in surface.findings]
        ok = (
            surface.exit_code() == EXIT_FINDINGS
            and got_where == [(RUNNER, want_line, "failure-mode")]
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the failure-mode finding names the report "
            f"step's own line and nothing else (reported {got_where}, want "
            f"[('{RUNNER}', {want_line}, 'failure-mode')])"
        )
        failures += 0 if ok else 1

        total += 1
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            got = report(_fixture(base, "printed", {MAKEFILE: UNPROTECTED_MAKEFILE}))
        output = printed.getvalue()
        ok = (
            got == EXIT_FINDINGS
            and "coverage-surface-check: FINDING Makefile:2: object-restriction:" in output
            and "a package selector does " in output
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the whole check reports the unprotected "
            f"per-package command with its file, line, rule and reason (exit {got}, want "
            f"{EXIT_FINDINGS})"
        )
        failures += 0 if ok else 1

        total += 1
        whole = scan(_fixture(base, "whole", {MAKEFILE: WORKSPACE_ONE_COMMAND}))
        ok = (
            whole.exit_code() == EXIT_OK
            and not [f for f in whole.findings if f.rule in ("object-restriction", "boundary")]
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: a --workspace run doing tests and report "
            f"in one command is exempt from rules 1 and 2, with the failure mode the "
            f"only thing it has to satisfy (exit {whole.exit_code()}, want {EXIT_OK}, "
            f"findings {[str(f) for f in whole.findings]})"
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
                f"self-test {'ok' if ok else 'FAIL'}: a Makefile this process cannot read fails "
                f"closed (exit {got}, want {EXIT_UNREADABLE})"
            )
            failures += 0 if ok else 1
            (root / MAKEFILE).chmod(0o600)

    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total - skipped} cases, {skipped} skipped)")
    return EXIT_OK
