"""Measuring one package in a directory of its own."""

from __future__ import annotations

import sys
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import COV_DIR, EXIT_FAIL, EXIT_OK, EXIT_RUN_ERROR, EXIT_USAGE, REPORT, RunError
from scoped_judge import load_report, report_rows
from scoped_profiles import (
    CARGO_LLVM_COV_BUILD_KEY,
    CARGO_LLVM_COV_TARGET_KEY,
    clear_profile_buffers,
    drop_invalid_profraws,
    drop_selected_objects,
    instrumented_dir,
    prune_foreign_objects,
    resolved_instrumented_dir,
)
from scoped_run import hermetic_env, run_coverage_tests, write_coverage_report
from scoped_select import workspace_packages

# Where a one-package measurement builds, relative to the workspace root.
#
# A directory of its own per package, because the fix for the merge this mode
# exists for is not a cleverer object rule: it is a walk that cannot see
# anything it should not. The tool resolves its target directory in three
# branches, read from its source rather than guessed. An explicit target
# directory wins; otherwise the diagnostic subcommand reports the ordinary one;
# only a real run falls back to the instrumented subdirectory above. So the
# explicit override is the only value that is both effective and stated, and a
# per-package path under it is what keeps one package's instrumented binaries out
# of the next package's merge.
PACKAGE_COV_DIR_NAME = "llvm-cov-package"


# --- one package, under a directory of its own ----------------------------------


class PackageMeasurement(NamedTuple):
    """One package's report, and the verdict of the test run behind it.

    The two travel together because a red suite must not cost the report: this
    mode judges nothing, so the report is the output while the run's own status
    is the exit code, and either alone would lose half of what happened.
    """

    report: Path
    test_status: int


def package_report_path(name: str) -> Path:
    """A report path of this package's own, named after the package.

    Separate per package for the same reason every other path here is: one shared
    name would let the last run's report stand in for the others. Separate from
    the gate's own paths too, so a diagnostic can never overwrite the artifact a
    floor was judged against.
    """
    return COV_DIR / f"coverage-package-{name}.json"


def package_target_override(name: str) -> str:
    """This package's instrumented directory, relative to the workspace root.

    Relative on purpose. The tool joins an override onto its own current
    directory, and this file runs every command from the workspace root, so a
    relative value and the absolute one resolve identically and a stored path
    stays readable on another machine.
    """
    return f"target/{PACKAGE_COV_DIR_NAME}/{name}"


def package_env(name: str) -> dict[str, str]:
    """The environment this package's run builds from.

    The target directory is the override and the build directory is removed. Both
    halves are the tool's rule: with the target directory set and no build
    directory, it uses the same path for both. An inherited build directory would
    send the executables somewhere else from the buffers, and the two steps in
    between would then inspect a directory this run never wrote.
    """
    env = hermetic_env()
    env[CARGO_LLVM_COV_TARGET_KEY] = package_target_override(name)
    env.pop(CARGO_LLVM_COV_BUILD_KEY, None)
    return env


def run_package_coverage(name: str) -> PackageMeasurement:
    """Measure one package into its own instrumented directory and report.

    The gate's same test and report steps run here, with stale selected executables removed before tests and this package's own instrumented directory for isolation.

    A red suite does not cost the report, which is the output this mode exists
    for. The test step writes no report of its own, so the buffers of every test
    that ran are on disk when it ends, and the hygiene pass and the report step
    read them exactly as they do after a green run. The run's own status is
    returned beside the report because exit 0 would say the package measured
    clean; nothing is judged here, so the status is the run's and not a verdict.
    """
    env = package_env(name)
    report_path = package_report_path(name)
    clear_profile_buffers(env)
    drop_selected_objects(resolved_instrumented_dir(env), [name])
    test_status = EXIT_OK
    try:
        run_coverage_tests([name], None, env)
    except RunError as error:
        # `_run_or_raise` already reprinted the run's own output; the message is
        # repeated here so the log still says which step failed, and the buffers
        # the failed run left are this report's input.
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        test_status = EXIT_FAIL

    # The same boundary the gate owns, against the directory this run built into.
    directory = instrumented_dir(env)
    drop_invalid_profraws(directory)
    prune_foreign_objects(directory, [name])

    write_coverage_report(report_path, env)
    return PackageMeasurement(report_path, test_status)


def coverage_package_mode(name: str) -> int:
    """Measure one package and print its rows. Judges nothing, gates nothing.

    A diagnostic, and named as one. It exists because the tool's own per-package
    command reports per-file rows that are not what they say: the merge sees
    binaries belonging to other packages, and a union of objects moves a file's
    denominator. This fixes the merge and adds no verdict, so a low number here
    is a measurement to look at, not a failure and not a pass.

    A red suite still gets its report: the run step writes no report of its own,
    so the buffers of every test that ran are on disk, and the mode writes the
    report from them, prints its path, and ends with the run's own non-zero
    status rather than an absence. Only a report this run could not produce at
    all is a run error.

    The cost is stated rather than discovered: each package gets its own cold
    instrumented build the first time it is measured, because the ordinary warm
    build lives in a different directory and cannot be reused. It is under
    `target/`, which is already build output.
    """
    package = next((p for p in workspace_packages() if p["name"] == name), None)
    if package is None:
        print(
            f"scoped-runner: error: {name} is not a workspace package, so there "
            f"is nothing to measure. Workspace packages: "
            f"{', '.join(sorted(p['name'] for p in workspace_packages()))}",
            file=sys.stderr,
        )
        return EXIT_USAGE
    print(
        f"scoped-runner: measuring package {name} in its own instrumented "
        f"directory {package_target_override(name)}"
    )
    print(
        "scoped-runner: this mode judges nothing and gates nothing; its numbers "
        "are only as good as its object list, which is why the run prunes it"
    )
    try:
        measurement = run_package_coverage(name)
        doc = load_report(measurement.report)
    except RunError as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR
    rows = report_rows(doc)
    # A report holds every file any object contributed, and the objects here are
    # this package's. A file from another package would mean the directory did
    # not do its job, so it is printed rather than filtered out of the listing.
    prefix = f"{package['dir']}/"
    owned = sorted(rel for rel in rows if rel.startswith(prefix))
    print(f"scoped-runner: {len(owned)} file(s) of {name} in {measurement.report}")
    for rel in owned:
        row = rows[rel]
        print(
            f"scoped-runner: {rel} lines {row.lines_pct:.2f}% "
            f"functions {row.functions_pct:.2f}%"
        )
    strays = sorted(rel for rel in rows if not rel.startswith(prefix))
    if strays:
        print(
            f"scoped-runner: WARNING {len(strays)} reported file(s) belong to no "
            f"object of {name}, so this run's directory did not isolate it: "
            f"{', '.join(strays[:5])}",
            file=sys.stderr,
        )
    print(
        f"scoped-runner: no floor was compared and no verdict is reported; "
        f"{REPORT.name} is the gate's report and was not written"
    )
    if measurement.test_status != EXIT_OK:
        # The report covers every test that ran (the ci profile runs them all),
        # and the non-zero status is the run's own. Saying so keeps the exit code
        # from reading as a failure to produce the measurement.
        print(
            "scoped-runner: the instrumented test run failed, so this report is "
            "the output of a red suite and the mode ends with the run's status",
            file=sys.stderr,
        )
    return measurement.test_status
