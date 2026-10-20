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
from scoped_judge import exemption, load_report, report_rows, verdicts
from scoped_package import coverage_package_mode
from scoped_run import run_coverage, run_tests
from scoped_select import is_measureable, is_present, select, workspace_packages
from quick_terms import union_expression


# --- entry point ---------------------------------------------------------------


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Targeted tests and coverage for the files a change touches."
    )
    parser.add_argument("--base", default="master", help="ref to diff against (default: master)")
    parser.add_argument("--plan", action="store_true", help="print the selection and run nothing")
    parser.add_argument(
        "--tests-only",
        action="store_true",
        help="run the selected tests without instrumentation, and judge nothing",
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
        _, _, changed = select(args.base, False)
        present = [rel for rel in changed if is_present(rel)]
        terms, targets = owned_terms(present)
        exempt = {rel: exemption(rel) for rel in present if is_measureable(rel) or "tests" in rel.split("/")}
    except (RunError, OwnedLookupError) as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR
    files = [rel for rel, reason in exempt.items() if reason is None]
    packages = sorted({term.package for term in terms})
    expression = union_expression(terms) if terms else ""
    print(f"scoped-runner: {len(present)} changed Rust file(s); {len(terms)} test selection(s) in {len(packages)} package(s): {', '.join(packages) or '(none)'}")
    for rel in [r for r in changed if r not in present]:
        print(f"scoped-runner: SKIPPED DELETED {rel}")
    if not args.tests_only:
        for rel, reason in exempt.items():
            if reason:
                print(f"scoped-runner: EXEMPT {rel}: {reason}")

    if args.plan:
        print(f"  packages {' '.join(packages) or '(none)'}")
        print(f"  targets {' '.join(targets) or '(all)'}")
        print(f"  filter {expression or '(none)'}")
        for rel in files if not args.tests_only else []:
            print(f"  judge {rel}")
        return EXIT_OK

    if not terms:
        print("scoped-runner: no changed file owns a test; no test runs")
        if args.tests_only:
            return EXIT_OK
        rows: dict = {}
    else:
        if args.tests_only:
            # nextest's exit code is the verdict; this mode judges nothing.
            return run_tests(packages, expression, targets)
        try:
            run_coverage(packages, expression, targets=targets)
            rows = report_rows(load_report(REPORT))
        except RunError as error:
            print(f"scoped-runner: error: {error}", file=sys.stderr)
            return EXIT_RUN_ERROR

    for rel in files:
        row = rows.get(rel)
        if row is not None:
            # Function and region percentages are printed and compared against nothing.
            print(f"scoped-runner: {rel} lines {row.lines_pct:.2f}% functions {row.functions_pct:.2f}%")
    failures = verdicts(rows, files, LINE_FLOOR)
    for failure in failures:
        print(f"scoped-runner: FAIL {failure}", file=sys.stderr)
    if failures:
        print(f"scoped-runner: {len(failures)} of {len(files)} judged file(s) failed at {LINE_FLOOR:.2f}% lines", file=sys.stderr)
        return EXIT_FAIL
    print(f"scoped-runner: every judged file ({len(files)}) reached {LINE_FLOOR:.2f}% lines")
    return EXIT_OK


def owned_terms(files: list[str]) -> tuple[list, list[str]]:
    """The selection `make quick` uses: each file's own test modules, or its integration target."""
    import quick

    packages = workspace_packages()
    selection = quick.select([rel for rel in files if rel.endswith(".rs")], packages)
    for note in selection.notes:
        print(f"scoped-runner: {note}")
    return selection.terms, quick.target_flags(selection.terms, packages)
