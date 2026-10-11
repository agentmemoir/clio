"""Self-test cases for the coverage steps, one package and the report counts."""

from __future__ import annotations

import io
import os
import sys
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import REPORT, ROOT
from scoped_judge import _count, report_rows
from scoped_package import package_env, package_report_path, package_target_override
from scoped_profiles import (
    CARGO_LLVM_COV_BUILD_KEY,
    CARGO_LLVM_COV_TARGET_KEY,
    INSTRUMENTED_DIR_NAME,
    RAW_PROFILE_HEADER_BYTES,
    clear_profile_buffers,
    drop_invalid_profraws,
    instrumented_dir,
    resolved_instrumented_dir,
)
from scoped_run import _run_or_raise, run_coverage

from scoped_selftest_support import shared as globals


def run(check, raises) -> tuple:
    # --- one package, under an instrumented directory of its own -------------
    #
    # The point of the mode is the directory, not the package filter: the tool
    # builds a report's object list by walking the whole instrumented target
    # directory, a package selection does not narrow that walk, and the merge
    # unions what it finds. So a row read out of the shared directory is a
    # property of whatever builds happen to be lying around, and the cases below
    # pin the two things that make it not so.
    store_report = package_report_path("clio-store")
    ops_report = package_report_path("clio-ops")
    check("a package report is named after the package", store_report.name, "coverage-package-clio-store.json")
    check("two packages get two report paths", store_report != ops_report, True)
    check(
        "a package report cannot overwrite the gate's report",
        store_report == REPORT,
        False,
    )
    check(
        "a package report cannot overwrite a per-file report",
        store_report.parent == REPORT.parent and store_report != REPORT,
        True,
    )
    override = package_target_override("clio-store")
    check("the package directory is named after the package", override.endswith("/clio-store"), True)
    check(
        "the package directory is relative, so it reads the same on another machine",
        override.startswith("/"),
        False,
    )
    check(
        "the package directory is not the shared instrumented directory",
        INSTRUMENTED_DIR_NAME in override,
        False,
    )
    check(
        "the package directory is under target, which is already build output",
        override.startswith("target/"),
        True,
    )
    # The resolution has to come from the environment the run builds from, or a
    # caller that pinned the directory would be reading a different one from the
    # tool's and the two steps would inspect a directory the run never wrote.
    resolved = resolved_instrumented_dir({CARGO_LLVM_COV_TARGET_KEY: override})
    check(
        "the override in the run's environment is the directory resolved",
        resolved,
        (ROOT / override).resolve(),
    )
    # The tool uses the build directory when it is set and the target directory
    # when it is not, so an inherited one silently splits the two paths this run
    # writes to from the path it reads back.
    inherited_build = os.environ.get(CARGO_LLVM_COV_BUILD_KEY)
    try:
        os.environ[CARGO_LLVM_COV_BUILD_KEY] = "target/some-other-build"
        check(
            "an inherited build directory is dropped so the two paths cannot diverge",
            CARGO_LLVM_COV_BUILD_KEY in package_env("clio-store"),
            False,
        )
    finally:
        if inherited_build is None:
            os.environ.pop(CARGO_LLVM_COV_BUILD_KEY, None)
        else:
            os.environ[CARGO_LLVM_COV_BUILD_KEY] = inherited_build
    check(
        "the run's environment carries the package's own target directory",
        package_env("clio-store").get(CARGO_LLVM_COV_TARGET_KEY),
        override,
    )

    # A failing step has to carry its own output. An exit code alone is what left
    # a whole run's diagnosis inside a tool's message nobody printed.
    class _Failing:
        returncode = 1
        stdout = ""
        stderr = "error: invalid instrumentation profile data (file header is corrupt)"

    def failing_run(cmd, **kw):
        return _Failing()

    original_run_for_failure = globals()["subprocess"].run
    original_stderr_for_failure = sys.stderr
    failure_log = io.StringIO()
    try:
        globals()["subprocess"].run = failing_run
        sys.stderr = failure_log
        try:
            raised = raises(_run_or_raise, ["cargo", "llvm-cov", "report"], {}, "the report step failed")
        finally:
            sys.stderr = original_stderr_for_failure
    finally:
        globals()["subprocess"].run = original_run_for_failure
    check("a failing step is an error", raised, True)
    check(
        "a failing step's own output is printed",
        "file header is corrupt" in failure_log.getvalue(),
        True,
    )

    # A silent run is not evidence of a finished one. The instrumented build can
    # take minutes with nothing between the two count lines, and this repository
    # has already spent an incident on reading an empty log as a hang.
    class _Quiet:
        returncode = 0
        stdout = "\n".join(f"building line {n}" for n in range(1, 400))
        stderr = ""

    try:
        globals()["subprocess"].run = lambda cmd, **kw: _Quiet()
        tail_log = io.StringIO()
        original_stdout_for_tail = sys.stdout
        try:
            sys.stdout = tail_log
            _run_or_raise(["cargo", "llvm-cov", "report"], {}, "the report step failed")
        finally:
            sys.stdout = original_stdout_for_tail
    finally:
        globals()["subprocess"].run = original_run_for_failure
    tail_text = tail_log.getvalue()
    check("a successful run prints the tail of its own output", "building line 399" in tail_text, True)
    check("a successful run does not print its whole output", "building line 1\n" in tail_text, False)
    check("a successful run says it finished", "the run finished" in tail_text, True)

    # A profile-buffer clean that fails is the one place a stale number can get in
    # silently: the buffers survive, the merge picks them up, and the report claims
    # coverage for code that did not run. A non-zero exit is a refusal.
    class _CleanFails:
        returncode = 101
        stdout = ""
        stderr = "error: the profile data directory could not be read"

    original_run_for_clean = globals()["subprocess"].run
    try:
        globals()["subprocess"].run = lambda cmd, **kw: _CleanFails()
        clean_refused = raises(clear_profile_buffers, {})
    finally:
        globals()["subprocess"].run = original_run_for_clean
    check("a failing profile-buffer clean is an error", clean_refused, True)

    # The gate path must inspect the directory its own run built in. Resolving it
    # from the process environment instead reads whatever the shell happens to
    # hold, so a run that pins the directory would have its hygiene steps look at a
    # directory the tool never wrote. The two agree today, which is exactly why the
    # difference needs a case: the one-package mode has always passed its own
    # environment, and the gate path did not.
    step_envs: dict[str, Any] = {}

    def record_dir(env: dict[str, str] | None = None) -> Path:
        step_envs["dir"] = env
        return ROOT / "target" / "llvm-cov-target"

    original_gate_globals = {
        name: globals()[name]
        for name in (
            "clear_profile_buffers",
            "run_coverage_tests",
            "instrumented_dir",
            "drop_invalid_profraws",
            "prune_foreign_objects",
            "write_coverage_report",
        )
    }
    try:
        globals()["clear_profile_buffers"] = lambda env: step_envs.__setitem__("clear", env)
        globals()["run_coverage_tests"] = (
            lambda pkgs, filt=None, env=None: step_envs.__setitem__("test", env)
        )
        globals()["instrumented_dir"] = record_dir
        globals()["drop_invalid_profraws"] = lambda directory: step_envs.__setitem__(
            "buffers", directory
        )
        globals()["prune_foreign_objects"] = (
            lambda directory, selected: step_envs.__setitem__("prune", directory)
        )
        globals()["write_coverage_report"] = (
            lambda path, env=None: step_envs.__setitem__("report", env)
        )
        run_coverage(["clio-store"])
    finally:
        for name, value in original_gate_globals.items():
            globals()[name] = value
    check(
        "the gate path resolves its instrumented directory from its own run's "
        "environment, not the process's",
        step_envs.get("dir") is step_envs.get("test") and step_envs.get("dir") is not None,
        True,
    )
    check(
        "and both hygiene steps inspect the directory that resolution returned",
        (step_envs.get("buffers"), step_envs.get("prune")),
        (ROOT / "target" / "llvm-cov-target", ROOT / "target" / "llvm-cov-target"),
    )
    check(
        "the report step runs in the same environment as the test step",
        step_envs.get("report") is step_envs.get("test"),
        True,
    )

    # --- counts out of the report, not only percentages ----------------------
    def report_with(summary: dict[str, Any]) -> dict[str, Any]:
        return {
            "data": [
                {
                    "files": [
                        {"filename": str(ROOT / "crates/a/src/thing.rs"), "summary": summary}
                    ]
                }
            ]
        }

    measured = report_rows(
        report_with(
            {
                "lines": {"count": 43, "covered": 32, "percent": 74.42},
                "functions": {"count": 34, "covered": 23, "percent": 67.65},
            }
        )
    )
    row = measured["crates/a/src/thing.rs"]
    check(
        "a row carries the percentages first",
        (row.lines_pct, row.functions_pct),
        (74.42, 67.65),
    )
    check(
        "a row carries the denominators the percentages were computed from",
        (row.lines_count, row.functions_count),
        (43, 34),
    )
    # A missing count is a shape change, not a measurement of nothing. Dropping
    # the row would report the file as unmeasured and look like a scope gap;
    # storing zero would invent a denominator and make a percentage meaningless.
    unknown = report_rows(
        report_with(
            {
                "lines": {"percent": 74.42},
                "functions": {"percent": 67.65},
            }
        )
    )
    check("a row with percentages and no counts is kept", "crates/a/src/thing.rs" in unknown, True)
    check(
        "a missing count is unknown rather than zero",
        (unknown["crates/a/src/thing.rs"].lines_count,
         unknown["crates/a/src/thing.rs"].functions_count),
        (None, None),
    )
    # The existing skip path is unchanged: a row with no percentage cannot be
    # judged, so it is not a row at all.
    check(
        "a row with no percentage is still dropped",
        report_rows(
            report_with(
                {"lines": {"count": 43}, "functions": {"count": 34}}
            )
        ),
        {},
    )
    check(
        "a boolean is not a count",
        _count({"count": True}),
        None,
    )

    # --- the profile buffers that cannot be read -----------------------------
    #
    # The fixture is a real directory with real files, because the rule is about
    # file sizes on disk and a mock would agree with whichever implementation it
    # was written against.
    with tempfile.TemporaryDirectory(prefix="scoped-runner-profraw-") as scratch:
        profraw_root = Path(scratch)
        healthy = profraw_root / "clio-1234-5678_0.profraw"
        empty = profraw_root / "clio-1234-5678_1.profraw"
        truncated = profraw_root / "clio-1234-5678_2.profraw"
        nested_dir = profraw_root / "nested"
        nested_dir.mkdir()
        nested = nested_dir / "clio-1234-5678_3.profraw"
        healthy.write_bytes(b"\x00" * (RAW_PROFILE_HEADER_BYTES + 64))
        empty.write_bytes(b"")
        truncated.write_bytes(b"\x00" * (RAW_PROFILE_HEADER_BYTES - 1))
        nested.write_bytes(b"")
        buffer_log = io.StringIO()
        original_stdout_for_buffers = sys.stdout
        try:
            sys.stdout = buffer_log
            checked, dropped = drop_invalid_profraws(profraw_root)
        finally:
            sys.stdout = original_stdout_for_buffers
        printed = buffer_log.getvalue()
        check("every buffer at the top level is checked", checked, 3)
        check("both unreadable buffers are dropped", dropped, 2)
        check("a buffer with a full header survives", healthy.exists(), True)
        check("an empty buffer is dropped", empty.exists(), False)
        check("a buffer one byte short of the header is dropped", truncated.exists(), False)
        # Only the top level is read by the merge, so a buffer the merge never
        # sees is not this step's to delete.
        check("a nested buffer is left alone", nested.exists(), True)
        check("both dropped buffers are named by path", printed.count("dropped unreadable profile buffer"), 2)
        check("a dropped buffer's size is printed", f"({RAW_PROFILE_HEADER_BYTES - 1} bytes" in printed, True)
        check(
            "the counts are printed whether or not anything was dropped",
            "profile buffers checked 3, dropped 2" in printed,
            True,
        )
        # Nothing dropped is the common case and still has to be visible, because
        # the rate of a dropped buffer is not measured.
        clean_log = io.StringIO()
        original_stdout_for_clean = sys.stdout
        try:
            sys.stdout = clean_log
            checked_clean, dropped_clean = drop_invalid_profraws(profraw_root)
        finally:
            sys.stdout = original_stdout_for_clean
        check(
            "a run that drops nothing still prints both counts",
            (checked_clean, dropped_clean, "dropped 0" in clean_log.getvalue()),
            (1, 0, True),
        )

    # A directory that does not exist is a refusal, not "zero buffers". Reading
    # zero buffers is exactly what a wrong path produces.
    missing_override = os.environ.get("CARGO_LLVM_COV_TARGET_DIR")
    try:
        os.environ["CARGO_LLVM_COV_TARGET_DIR"] = "target/definitely-not-here"
        check(
            "an instrumented directory that does not exist is an error",
            raises(instrumented_dir),
            True,
        )
    finally:
        if missing_override is None:
            os.environ.pop("CARGO_LLVM_COV_TARGET_DIR", None)
        else:
            os.environ["CARGO_LLVM_COV_TARGET_DIR"] = missing_override
    return name, scratch, value
