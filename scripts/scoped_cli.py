"""The command line of the scoped runner."""

from __future__ import annotations

import argparse
import sys

from scoped_core import (
    EXIT_FAIL,
    EXIT_OK,
    EXIT_RUN_ERROR,
    LINE_FLOOR,
    OwnedLookupError,
    REPORT,
    RunError,
)
from scoped_judge import (
    file_scope_filter,
    judge,
    judge_file_scope,
    load_report,
    renames,
    report_rows,
    report_verdicts,
)
from scoped_package import coverage_package_mode
from scoped_run import run_coverage, run_tests
from scoped_select import (
    build_inputs,
    changed_paths,
    is_measureable,
    is_present,
    parse_codeless,
    select,
    workspace_packages,
)


# --- entry point ---------------------------------------------------------------


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Targeted tests and coverage for the files a change touches."
    )
    parser.add_argument("--base", default="master", help="ref to diff against (default: master)")
    parser.add_argument("--all", action="store_true", help="measure every package")
    parser.add_argument("--plan", action="store_true", help="print the selection and run nothing")
    parser.add_argument(
        "--tests-only",
        action="store_true",
        help="run the selected packages' tests without instrumentation, and judge nothing",
    )
    parser.add_argument(
        "--scope",
        choices=("crate", "file"),
        default="crate",
        help=(
            "crate: run every selected package's tests. "
            "file: run only the changed files' own tests. Both judge the same "
            f"{LINE_FLOOR:.0f}%% line floor (default: crate)"
        ),
    )
    parser.add_argument(
        "--self-test", action="store_true", help="run this script's own tests"
    )
    parser.add_argument(
        "--coverage-package",
        default=None,
        metavar="NAME",
        help=(
            "measure one package under an instrumented directory of its own and "
            "print its per-file rows. Compares nothing, judges nothing"
        ),
    )
    args = parser.parse_args(argv)
    if args.self_test:
        from scoped_selftest import self_test

        return self_test()

    if args.coverage_package is not None:
        return coverage_package_mode(args.coverage_package)

    try:
        packages, files, changed = select(args.base, args.all)
    except RunError as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR

    label = "test" if args.tests_only else "judge"
    codeless = parse_codeless(changed)
    no_code = [rel for rel in changed if rel in codeless and is_present(rel)]
    print(f"scoped-runner: scope={args.scope}")
    print(f"scoped-runner: {len(files)} changed file(s) to {label}")
    if no_code:
        # Printed rather than silently dropped: the gate's contract is that the
        # reader can see every file the run did not judge. The source decided it
        # holds no counted line, so the run names it and moves on.
        for rel in no_code:
            print(f"scoped-runner: NO EXECUTABLE CODE {rel}")
    print(f"scoped-runner: {len(packages)} package(s) to run: {', '.join(packages) or '(none)'}")
    # Files that are not Rust but decide what a crate compiles or reads (issue #69). They
    # select packages in `select`; here they only get named, and they turn off the file filter
    # below, because a changed lock file or fixture can break any test of the packages it reaches.
    build_files = [] if args.all else build_inputs(changed_paths(args.base), workspace_packages())[1]
    for rel in build_files:
        print(f"scoped-runner: BUILD INPUT {rel}")

    file_filter = ""
    unplanned: list[str] = []
    if args.scope == "file" and not build_files:
        # A deleted file owns no tests because nothing is left of it, and the
        # ownership lookup cannot reach a path that no crate root holds any more.
        # It is skipped and named rather than raised on, so a change that retires
        # a test module can still be measured for the files that remain.
        for rel in [r for r in changed if not is_present(r)]:
            print(f"scoped-runner: SKIPPED DELETED {rel}")
        try:
            # The filter reads `changed`, not `files`, so a test under a `tests/`
            # folder still runs even though it can never be measured. Judging
            # below reads `files`, which excludes it. Splitting the two here is
            # what keeps a new integration test from being run by nothing. The
            # deleted paths are the one thing filtered out, because asking the
            # lookup about a file that is gone is the failure this avoids.
            file_filter, unplanned = file_scope_filter([r for r in changed if is_present(r)])
        except OwnedLookupError as error:
            print(f"scoped-runner: error: {error}", file=sys.stderr)
            return EXIT_RUN_ERROR
        # A path-excluded file is not a file that owes the codebase a test module.
        # Complaining about one would fail the run for a file the report could
        # never hold in the first place, which is the failure being fixed here.
        unplanned = [rel for rel in unplanned if is_measureable(rel)]
        for rel in unplanned:
            print(f"scoped-runner: NO OWN TESTS {rel}", file=sys.stderr)
        if not file_filter:
            # Every changed file owns no test, so there is nothing to run. Saying
            # so is the honest outcome; a filter matching nothing would exit
            # non-zero and look like a test failure.
            print("scoped-runner: no changed file owns a test; nothing to run")
            if args.tests_only or not files:
                # Nothing to run and nothing to judge. Every changed file is
                # path-excluded, holds no executable code, or is gone from the
                # tree, so there is no verdict to give. Exiting non-zero
                # here reported a failure over zero files, which is the empty-scope
                # lie the run refuses to tell elsewhere.
                return EXIT_OK
            # Judging mode still has a verdict to give: these files are at 0%.
            for rel in files:
                print(f"scoped-runner: BELOW FLOOR {rel} lines 0.00%")
            return EXIT_FAIL
        print(f"scoped-runner: file-scoped filter over {len(changed)} file(s)")

    if args.plan:
        for rel in files:
            print(f"  {label} {rel}")
        if file_filter:
            print(f"  filter {file_filter}")
        return EXIT_OK

    # Nothing to measure and nothing to judge. Reaching here means the tree is
    # clean of Rust changes: `select` raises when the tree is dirty but no Rust
    # file was found, so an empty list here is a real all-clear rather than a
    # selection that silently failed.
    if not packages:
        print("scoped-runner: no changed Rust file; nothing to run")
        return EXIT_OK

    if build_files and not files and not args.tests_only:
        # Only build inputs changed, so no Rust file is owed a number. The test step runs the
        # packages' tests; an instrumented run of all of them would judge nothing.
        print("scoped-runner: only build inputs changed; no Rust file to judge")
        return EXIT_OK

    if args.tests_only:
        # A changed file whose tests cannot be located is a failure in this mode
        # too. Returning 0 here reported a green fast loop on a change whose only
        # new test never ran, which is the exact scenario the crate scope was
        # patched for earlier: a commit that adds a test must run it.
        if args.scope == "file" and unplanned:
            for rel in unplanned:
                print(
                    f"scoped-runner: FAIL {rel} owns no test module, so no test of "
                    f"its own ran; migrate its tests to the convention",
                    file=sys.stderr,
                )
            print(
                f"scoped-runner: {len(unplanned)} changed file(s) own no test",
                file=sys.stderr,
            )
            return EXIT_FAIL
        # nextest's exit code is the verdict. A failure is reported by the test
        # runner itself, with its own name and output, so this tool does not
        # summarise or reinterpret it.
        code = run_tests(packages, file_filter or None)
        if code != 0:
            return code
        return EXIT_OK

    try:
        if args.scope == "file":
            # One instrumented pass per changed file, so each file's coverage is
            # produced by its own tests and nothing else. The judgement is the
            # same one, at the same number; only the test set differs.
            offenders, unreported = judge_file_scope(packages, files, LINE_FLOOR)
            print(
                f"scoped-runner: judged {len(files)} file(s) at "
                f"{LINE_FLOOR:.2f}% lines, each from its own tests"
            )
            if offenders or unreported:
                # Named before the count, because a failing run that cannot be
                # acted on from its own log has not reported anything useful.
                report_verdicts(offenders, unreported, sys.stderr)
                print(
                    f"scoped-runner: {len(offenders)} below floor, "
                    f"{len(unreported)} not measured",
                    file=sys.stderr,
                )
                return EXIT_FAIL
            print(
                "scoped-runner: every judged file meets the floor from its own "
                f"tests ({len(files)} judged, {len(changed) - len(files)} "
                "excluded from coverage: path-excluded by the tool, or holding "
                "no executable code)"
            )
            return EXIT_OK

        run_coverage(packages, file_filter or None)
        doc = load_report(REPORT)
        rows = report_rows(doc)
    except RunError as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR

    offenders, unreported = judge(rows, files, LINE_FLOOR)

    measured = len(files) - len(unreported)
    print(f"scoped-runner: {measured}/{len(files)} changed file(s) present in the report")
    # A file this change only moved is judged exactly as any other changed file
    # is. This line says so, because a reader who renamed a file and then read a
    # coverage verdict for it would otherwise have no way to tell a file whose
    # content this change left alone from one it never touched.
    try:
        moved = renames(args.base)
    except RunError:
        moved = set()
    for rel in files:
        if rel in moved:
            print(
                f"scoped-runner: MOVED WITHOUT EDIT {rel}: this change renamed it "
                "and left its content alone, so it adds no line. Its coverage is "
                "whatever it was before the move, and it is judged on that"
            )
    report_verdicts(offenders, unreported, sys.stdout)

    if offenders or unreported:
        print(
            f"scoped-runner: {len(offenders)} below floor, {len(unreported)} not "
            f"measured, at {LINE_FLOOR:.2f}% lines",
            file=sys.stderr,
        )
        return EXIT_FAIL
    print(
        f"scoped-runner: every measured changed file reached {LINE_FLOOR:.2f}% lines"
    )
    return EXIT_OK
