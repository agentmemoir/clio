"""Self-test cases for the one-package mode wired end to end, and the last rules."""

from __future__ import annotations

import io
import sys
import tempfile
from pathlib import Path, PurePosixPath

from scoped_runner import (
    CARGO_LLVM_COV_TARGET_KEY,
    CoverageRow,
    EXIT_FAIL,
    EXIT_OK,
    EXIT_RUN_ERROR,
    EXIT_USAGE,
    RAW_PROFILE_HEADER_BYTES,
    ROOT,
    RunError,
    _inside,
    judge,
    main,
    package_report_path,
    package_target_override,
    renames,
    resolved_instrumented_dir,
)

from scoped_selftest_support import shared as globals


def run(check, name, prune_packages) -> None:
    # --- the one-package mode, wired end to end without a build --------------
    #
    # Every step is recorded rather than run, because what is under test is the
    # order, the directory, the package, and the report path: four things that a
    # real run would only prove by producing the wrong rows and taking an hour.
    calls: list[tuple] = []
    original_mode_globals = {
        name: globals()[name]
        for name in (
            "workspace_packages",
            "clear_profile_buffers",
            "run_coverage_tests",
            "instrumented_dir",
            "drop_invalid_profraws",
            "prune_foreign_objects",
            "write_coverage_report",
            "load_report",
        )
    }
    one_package_report = {
        "data": [
            {
                "files": [
                    {
                        "filename": str(ROOT / "crates/clio-store/src/sqlite.rs"),
                        "segments": [],
                        "summary": {
                            "lines": {"count": 10, "covered": 9, "percent": 90.0},
                            "functions": {"count": 4, "covered": 3, "percent": 75.0},
                        },
                    },
                    # A file of another package. It cannot happen when the
                    # directory does its job, so if it appears in the report the
                    # directory did not, and a mode whose whole point is a
                    # correct object list has to say so rather than print rows.
                    {
                        "filename": str(ROOT / "crates/clio-ops/src/mod.rs"),
                        "segments": [],
                        "summary": {
                            "lines": {"count": 5, "covered": 1, "percent": 20.0},
                            "functions": {"count": 2, "covered": 1, "percent": 50.0},
                        },
                    },
                ]
            }
        ]
    }
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        globals()["clear_profile_buffers"] = lambda env: calls.append(("clear", env))
        globals()["run_coverage_tests"] = lambda pkgs, filt=None, env=None: calls.append(
            ("test", tuple(pkgs), dict(env or {}))
        )
        # The real one refuses a directory that does not exist, which is the
        # right behaviour and impossible here, so it is replaced by its own
        # resolution: what is under test is WHICH directory the mode picked, and
        # that it picked it from its own run's environment.
        globals()["instrumented_dir"] = lambda env=None: (
            calls.append(("dir", resolved_instrumented_dir(env))) or calls[-1][1]
        )
        globals()["drop_invalid_profraws"] = lambda directory: calls.append(("buffers", Path(directory)))
        globals()["prune_foreign_objects"] = lambda directory, selected: calls.append(
            ("prune", Path(directory), tuple(selected))
        ) or 0
        globals()["write_coverage_report"] = lambda path, env=None: calls.append(("report", Path(path)))
        globals()["load_report"] = lambda path: one_package_report
        original_stdout_for_mode = sys.stdout
        original_stderr_for_mode = sys.stderr
        mode_out, mode_err = io.StringIO(), io.StringIO()
        try:
            sys.stdout = mode_out
            sys.stderr = mode_err
            package_code = main(["--coverage-package", "clio-store"])
        finally:
            sys.stdout = original_stdout_for_mode
            sys.stderr = original_stderr_for_mode
        check("the one-package mode reports and does not judge", package_code, EXIT_OK)
        check(
            "it runs the same steps in the same order",
            [call[0] for call in calls],
            ["clear", "test", "dir", "buffers", "prune", "report"],
        )
        check("its test step names exactly this package", calls[1][1], ("clio-store",))
        expected_directory = (ROOT / package_target_override("clio-store")).resolve()
        check(
            "it resolves the package's own instrumented directory",
            calls[2][1],
            expected_directory,
        )
        check(
            "its hygiene steps run against that directory, not the shared one",
            (calls[3][1], calls[4][1]),
            (expected_directory, expected_directory),
        )
        check(
            "its report path is the package's own, not the gate's",
            calls[5][1],
            package_report_path("clio-store"),
        )
        check(
            "its tools build into that directory, not the shared one",
            calls[1][2].get(CARGO_LLVM_COV_TARGET_KEY),
            package_target_override("clio-store"),
        )
        check(
            "it prunes the object list rather than trusting the directory",
            calls[4][2],
            ("clio-store",),
        )
        check(
            "it prints a row for the package's own file",
            "crates/clio-store/src/sqlite.rs lines 90.00% functions 75.00%" in mode_out.getvalue(),
            True,
        )
        check(
            "a file of another package is a warning, not a silent row",
            "did not isolate it" in mode_err.getvalue(),
            True,
        )
        check(
            "it says it compared no floor",
            "no verdict is reported" in mode_out.getvalue(),
            True,
        )
    finally:
        for name, value in original_mode_globals.items():
            globals()[name] = value

    # The same mode on a red suite. The run step records its package and then
    # fails the way a failing test does, and every later step still has to
    # happen: the report is the output, so it is written from the buffers that
    # run left, and the exit code carries the run's verdict rather than taking
    # the report away.
    red_calls: list[tuple] = []
    original_red_globals = {
        name: globals()[name]
        for name in (
            "workspace_packages",
            "clear_profile_buffers",
            "run_coverage_tests",
            "instrumented_dir",
            "drop_invalid_profraws",
            "prune_foreign_objects",
            "write_coverage_report",
            "load_report",
        )
    }

    def _failing_test_step(pkgs, filt=None, env=None) -> None:
        red_calls.append(("test", tuple(pkgs)))
        raise RunError("the instrumented test run failed (exit 100)")

    try:
        globals()["workspace_packages"] = lambda: prune_packages
        globals()["clear_profile_buffers"] = lambda env: red_calls.append(("clear", env))
        globals()["run_coverage_tests"] = _failing_test_step
        globals()["instrumented_dir"] = lambda env=None: (
            red_calls.append(("dir", resolved_instrumented_dir(env))) or red_calls[-1][1]
        )
        globals()["drop_invalid_profraws"] = lambda directory: red_calls.append(
            ("buffers", Path(directory))
        )
        globals()["prune_foreign_objects"] = lambda directory, selected: red_calls.append(
            ("prune", Path(directory), tuple(selected))
        ) or 0
        globals()["write_coverage_report"] = lambda path, env=None: red_calls.append(
            ("report", Path(path))
        )
        globals()["load_report"] = lambda path: one_package_report
        original_stdout_for_red = sys.stdout
        original_stderr_for_red = sys.stderr
        red_out, red_err = io.StringIO(), io.StringIO()
        try:
            sys.stdout = red_out
            sys.stderr = red_err
            red_code = main(["--coverage-package", "clio-store"])
        finally:
            sys.stdout = original_stdout_for_red
            sys.stderr = original_stderr_for_red
        check(
            "a red suite runs the same steps in the same order",
            [call[0] for call in red_calls],
            ["clear", "test", "dir", "buffers", "prune", "report"],
        )
        check(
            "a red suite still writes the package's own report",
            red_calls[-1][1],
            package_report_path("clio-store"),
        )
        check(
            "the failed run is named",
            "the instrumented test run failed" in red_err.getvalue(),
            True,
        )
        check(
            "the report is printed even though the run failed",
            "crates/clio-store/src/sqlite.rs lines 90.00% functions 75.00%"
            in red_out.getvalue(),
            True,
        )
        check("a red suite ends with the run's non-zero status", red_code, EXIT_FAIL)
    finally:
        for name, value in original_red_globals.items():
            globals()[name] = value

    # A name that is not a package is a usage error and must build nothing: an
    # empty measurement that prints a header and exits 0 is the failure this
    # avoids.
    unknown_log = io.StringIO()
    original_stderr_for_unknown = sys.stderr
    original_packages_for_unknown = globals()["workspace_packages"]
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        sys.stderr = unknown_log
        check(
            "an unknown package is a usage error",
            main(["--coverage-package", "clio-nonexistent"]),
            EXIT_USAGE,
        )
    finally:
        sys.stderr = original_stderr_for_unknown
        globals()["workspace_packages"] = original_packages_for_unknown
    check("the unknown package is named", "clio-nonexistent" in unknown_log.getvalue(), True)

    # A measurement that could not be produced is exit 3. Exit 0 would say the
    # package measured clean, which is the opposite of what happened.
    def _raising_package_run(name: str) -> Path:
        raise RunError("the instrumented build directory does not exist")

    original_run_for_package_error = globals()["run_package_coverage"]
    error_log = io.StringIO()
    original_stderr_for_package_error = sys.stderr
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        globals()["run_package_coverage"] = _raising_package_run
        sys.stderr = error_log
        check(
            "a measurement that could not be produced is a run error",
            main(["--coverage-package", "clio-store"]),
            EXIT_RUN_ERROR,
        )
    finally:
        sys.stderr = original_stderr_for_package_error
        globals()["run_package_coverage"] = original_run_for_package_error
        globals()["workspace_packages"] = original_packages_for_unknown

    # A deletion is only ever allowed inside the instrumented directory. The
    # check exists because a mistake in resolving that directory would otherwise
    # reach an ordinary build directory.
    with tempfile.TemporaryDirectory(prefix="scoped-runner-boundary-") as scratch:
        boundary_root = Path(scratch)
        inside_dir = boundary_root / "llvm-cov-target"
        inside_dir.mkdir()
        escapee = boundary_root / "outside.profraw"
        escapee.write_bytes(b"")
        check(
            "a path outside the instrumented directory is not inside it",
            _inside(escapee, inside_dir),
            False,
        )
        check(
            "a path inside the instrumented directory is inside it",
            _inside(inside_dir / "a.profraw", inside_dir),
            True,
        )

    # The header size is a measured constant and not a plausible-looking number,
    # so its own value is asserted. If it changes, the assertion is what notices.
    check("the raw profile header size is measured", RAW_PROFILE_HEADER_BYTES, 128)

    # A pure rename adds no line because nothing about the file changed, which is a
    # different fact from this change never having touched it. It is a diagnostic
    # and changes no verdict, so these cases pin the parse and nothing else.
    original_base_for_none = globals()["merge_base"]
    original_git_for_renames = globals()["git"]
    try:
        globals()["merge_base"] = lambda base: None
        check("a change with no merge base reports no renames", renames("master"), set())
        globals()["merge_base"] = lambda base: "abc123"
        globals()["git"] = lambda *a: "R100\0crates/a/src/old.rs\0crates/a/src/new.rs\0"
        check("a pure rename names its new path", renames("master"), {"crates/a/src/new.rs"})
        globals()["git"] = lambda *a: "M\0crates/a/src/edited.rs\0"
        check("an edit is not a rename", renames("master"), set())
    finally:
        globals()["merge_base"] = original_base_for_none
        globals()["git"] = original_git_for_renames

    # The gate itself, in both directions, at the one number. These are the cases
    # that hold the single rule in place, and they are deliberately about the
    # percentage alone: the counts a report also carries are read and printed, and
    # nothing compares them, so a row whose covered count disagrees with its own
    # percentage cannot change a verdict.
    counted = judge(
        {
            # 3 of 191 lines covered is 1.57%, and the row says 90%. The
            # percentage is what the rule reads, so this fails on the percentage
            # and the disagreement in the counts is invisible to the verdict.
            "crates/a/src/one.rs": CoverageRow(90.0, 100.0, 191, 24, 3, 24),
        },
        ["crates/a/src/one.rs"],
    )[0]
    check("the line percentage is the only input", len(counted), 0)
    below = judge(
        {"crates/a/src/two.rs": CoverageRow(79.99, 0.0)}, ["crates/a/src/two.rs"]
    )[0]
    check("a file under the floor fails", len(below), 1)
    check(
        "the message quotes the one number and nothing else",
        below[0],
        "crates/a/src/two.rs lines 79.99% (floor 80.00)",
    )
    check(
        "function coverage never appears in a failure",
        any("function" in line for line in below),
        False,
    )
    check(
        "a file at 0% functions passes when its lines clear the floor",
        judge({"crates/a/src/three.rs": CoverageRow(100.0, 0.0)}, ["crates/a/src/three.rs"])[0],
        [],
    )
    check(
        "a file at 0% lines fails even at 100% functions",
        len(judge({"crates/a/src/four.rs": CoverageRow(0.0, 100.0)}, ["crates/a/src/four.rs"])[0]),
        1,
    )
    mixed_offenders, mixed_missing = judge(
        {
            "crates/a/src/good.rs": CoverageRow(95.0, 100.0),
            "crates/a/src/thin.rs": CoverageRow(70.0, 100.0),
        },
        ["crates/a/src/good.rs", "crates/a/src/thin.rs", "crates/a/src/absent.rs"],
    )
    check("only the file below the floor is an offender", len(mixed_offenders), 1)
    check("the offender is the thin one", "thin.rs" in mixed_offenders[0], True)
    check(
        "the file the report lacks is named, not judged",
        mixed_missing,
        ["crates/a/src/absent.rs"],
    )
    check(
        "a caller cannot pass a second, competing number",
        judge({"crates/a/src/five.rs": CoverageRow(79.0, 0.0)}, ["crates/a/src/five.rs"], 70.0)[0],
        [],
    )
