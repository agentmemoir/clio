#!/usr/bin/env python3
"""Tests for scripts/phase_timer.py and scripts/phase_report.py.

Run: python3 scripts/test_phase_timer.py

Why these exist
---------------
The first version of the phase timer measured a run correctly and then threw the
measurement away: it wrote none of the child's output, kept only the ten slowest
tests in its JSON, and printed no failing test. A later run exited 100 with real
failures and the report showed only passing tests. Every assertion below exists
to pin one of those losses shut, so the tool cannot quietly go back to
discarding evidence.

The sampler and host-probe tests are in scripts/test_phase_timer_sampling.py and
are imported here, so this one command runs the whole phase-timer suite.
"""

from __future__ import annotations

import subprocess
import sys
import unittest

from phase_timer_testkit import FAKE_RUN, TIMER, run_timer

from phase_report import Report, human

# Imported so `unittest.main()` below discovers them: the loader reads the
# `__main__` module's globals, and an imported TestCase is a global.
from test_phase_timer_sampling import (  # noqa: F401
    FindsWhereExecutionIsRefused,
    MeasuresTheKernelNotJustTheOutput,
    RecognisesWhatTheHostSupports,
)


class CapturesEveryTiming(unittest.TestCase):
    """Every timing is recorded, not sampled."""

    def test_every_test_result_is_kept_not_just_the_slowest(self):
        _, _, report, _ = run_timer(FAKE_RUN)
        self.assertEqual(3, len(report["all_tests"]), "all three results must be present")
        # nextest prints `<binary-id> <test-path>`; the test path is kept whole so
        # a CI filter that selects on a name substring keeps working.
        self.assertEqual(
            ["clio::bin/clio alpha::one", "clio::bin/clio alpha::two", "clio-store beta::three"],
            [t["name"] for t in report["all_tests"]],
        )
        self.assertEqual(
            sorted(t["seconds"] for t in report["all_tests"]),
            [0.25, 0.75, 1.5],
        )

    def test_every_output_line_is_kept_with_its_arrival_offset(self):
        _, _, report, _ = run_timer(FAKE_RUN)
        lines = report["all_lines"]
        self.assertEqual(report["child_lines"], len(lines))
        self.assertGreaterEqual(len(lines), 8, "every child line must be recorded")
        offsets = [line["at_offset"] for line in lines]
        self.assertEqual(offsets, sorted(offsets), "offsets must be in arrival order")
        self.assertTrue(any("Compiling" in line["text"] for line in lines))

    def test_every_silent_interval_is_kept_individually(self):
        _, _, report, _ = run_timer(FAKE_RUN)
        # One gap before the first line, one between each pair, one at teardown.
        self.assertGreaterEqual(len(report["all_gaps"]), len(report["all_lines"]))
        self.assertTrue(all(g["seconds"] >= 0 for g in report["all_gaps"]))
        self.assertTrue(report["gap_summary"], "gaps must also be merged for reading")

    def test_lock_wait_is_not_swallowed_by_the_lock_wait_phase(self):
        _, stderr, report, _ = run_timer(FAKE_RUN)
        self.assertTrue(report["lock_wait_observed"])
        self.assertIn("build-lock contention", stderr)

    def test_cargo_reported_compile_time_is_recorded(self):
        _, _, report, _ = run_timer(FAKE_RUN)
        self.assertEqual(90.0, report["cargo_compile_seconds"])

    def test_residual_accounts_for_the_whole_run(self):
        _, _, report, _ = run_timer(FAKE_RUN)
        self.assertLess(report["residual_seconds"], 0.5)
        self.assertAlmostEqual(
            report["wall_seconds"],
            report["residual_seconds"] + min(report["wall_seconds"], report["attributed_seconds"] if "attributed_seconds" in report else report["wall_seconds"]),
            delta=0.5,
        )


class SurfacesFailures(unittest.TestCase):
    """A red suite cannot hide behind a green-looking report."""

    def test_child_exit_code_is_propagated(self):
        code, _, _, _ = run_timer(FAKE_RUN)
        self.assertEqual(100, code, "the timer must proxy the child's exit code")

    def test_failing_test_is_named_in_the_report(self):
        _, stderr, report, _ = run_timer(FAKE_RUN)
        self.assertEqual(1, len(report["non_pass"]))
        self.assertEqual("FAIL", report["non_pass"][0]["status"])
        self.assertIn("beta::three", report["non_pass"][0]["name"])
        self.assertIn("NON-PASS", stderr)
        self.assertIn("beta::three", stderr)

    def test_raw_child_output_is_written_to_the_log(self):
        _, _, _, raw = run_timer(FAKE_RUN)
        self.assertIn("Compiling clio", raw)
        self.assertIn("FAIL", raw, "the raw log is the last place a failure can hide")

    def test_nonzero_exit_without_a_captured_failure_says_so(self):
        code, stderr, _, _ = run_timer("echo nothing-useful; exit 3")
        self.assertEqual(3, code)
        self.assertIn("no failing result line was captured", stderr)

    def test_status_counts_cover_every_result(self):
        _, _, report, _ = run_timer(FAKE_RUN)
        self.assertEqual({"PASS": 2, "FAIL": 1}, report["status_counts"])


class ReportsPartialCoverage(unittest.TestCase):
    """A partial picture is labelled as partial, never as the whole run."""

    def test_missing_result_lines_are_counted_and_explained(self):
        # Summary claims 900 tests; only two result lines are printed, which is
        # what nextest does without --status-level all.
        script = (
            'echo "        PASS [   1.000s] ( 1/2) clio::bin/clio a::one"\n'
            'echo "        PASS [   1.000s] ( 2/2) clio::bin/clio a::two"\n'
            'echo "  Summary [   2.000s] 900 tests run: 900 passed"\n'
        )
        _, stderr, report, _ = run_timer(script)
        self.assertEqual(900, report["tests_run"])
        self.assertEqual(898, report["tests_missing_from_report"])
        self.assertIn("--status-level all", stderr)

    def test_complete_run_reports_no_missing_lines(self):
        _, stderr, report, _ = run_timer(FAKE_RUN)
        self.assertEqual(0, report["tests_missing_from_report"])
        self.assertNotIn("--status-level all", stderr)


class GroupsSlowTestsByPackage(unittest.TestCase):
    """A slow crate is attributable without re-parsing names by hand."""

    def test_per_package_totals_and_maxima(self):
        script = (
            'echo "        PASS [   9.000s] ( 1/3) clio-store heavy::one"\n'
            'echo "        PASS [   1.000s] ( 2/3) clio-store light::two"\n'
            'echo "        PASS [   0.500s] ( 3/3) clio-mcp quick::three"\n'
        )
        _, _, report, _ = run_timer(script)
        rows = {row["binary"]: row for row in report["by_binary"]}
        self.assertEqual(10.0, rows["clio-store"]["total_seconds"])
        self.assertEqual(9.0, rows["clio-store"]["max_seconds"])
        self.assertEqual(2, rows["clio-store"]["count"])
        self.assertEqual(0.5, rows["clio-mcp"]["total_seconds"])

    def test_totals_are_ordered_heaviest_first(self):
        script = "".join(
            f'echo "        PASS [   {n}.000s] ( 1/2) pkg{n} t::{n}"\n' for n in (1, 8)
        ) + 'echo "  Summary [   9.000s] 2 tests run: 2 passed"\n'
        _, _, report, _ = run_timer(script)
        self.assertEqual("pkg8", report["by_binary"][0]["binary"])


class NamesEveryStage(unittest.TestCase):
    """Every stage of a run is named, including the ones that print nothing.

    A stage that prints no line is the expensive kind: it cannot be recovered
    from the output, so an unnamed one is a span someone later has to guess at.
    These pin the full set, so a new silent stage cannot appear unnamed.
    """

    #: A cargo+nextest run with one of every stage this timer knows about.
    # NOTE: the backticks are escaped. Inside a double-quoted sh string a bare
    # backtick starts command substitution, so an unescaped one makes the fixture
    # emit "  Updating  index" and silently test the wrong line.
    FULL_RUN = (
        'echo "    Blocking waiting for file lock on build directory"\n'
        'echo "  Updating \\`crates-io\\` index"\n'
        'echo " Downloading crates ..."\n'
        'echo "  Locking 400 packages"\n'
        'echo " Adding serde v1.0.0 to dependency graph"\n'
        'echo "   Compiling clio v0.1.0 (/repo/crates/clio)"\n'
        'echo "    Fresh clio-types v0.1.0"\n'
        'echo "    Finished `test` profile [unoptimized + debuginfo] target(s) in 1m 30s"\n'
        'echo "  Starting 1 tests across 1 binary (0 tests skipped)"\n'
        'echo "        PASS [   0.100s] ( 1/1) clio::bin/clio a::one"\n'
        'echo "  Summary [   0.100s] 1 tests run: 1 passed"\n'
    )

    def test_every_known_stage_is_recognised(self):
        _, _, report, _ = run_timer(self.FULL_RUN)
        seen = {line["phase"] for line in report["all_lines"]}
        for stage in (
            "lock-wait",
            "fetch-index",
            "download",
            "fetch-lock",
            "resolve",
            "compile",
            "fresh",
            "build-done",
            "test-start",
            "tests",
            "summary",
        ):
            self.assertIn(stage, seen, f"stage {stage!r} was not recognised")

    def test_the_silent_post_build_span_is_named_not_filed_under_the_build(self):
        # The pause between "Finished" and "Starting" is nextest listing every
        # test binary. It must not be charged to the build, which is what the
        # old "compile-done (silent)" bucket did.
        _, stderr, report, _ = run_timer(self.FULL_RUN)
        causes = {gap["name"] for gap in report["all_gaps"]}
        self.assertIn("test-binary listing (silent)", causes)
        self.assertNotIn(
            "build-done (silent)",
            causes,
            "the post-build silence belongs to nextest, not to the compiler",
        )
        # And the span must be visible in the printed gap summary too.
        summary = stderr.split("silent gaps", 1)[1]
        self.assertIn("test-binary listing", summary)

    def test_every_silent_span_of_the_run_is_attributed_to_a_named_cause(self):
        _, _, report, _ = run_timer(self.FULL_RUN)
        # Nothing may be left to the residual, and every gap must carry a cause.
        self.assertLess(report["residual_seconds"], 0.5)
        for gap in report["all_gaps"]:
            self.assertTrue(gap["name"].strip(), "a gap with no cause is a guess waiting to happen")

    def test_spawn_and_teardown_are_both_named(self):
        _, stderr, report, _ = run_timer(self.FULL_RUN)
        causes = {gap["name"] for gap in report["all_gaps"]}
        self.assertTrue(
            any("timer startup" in c or "timer-to-spawn" in c for c in causes),
            f"the span before the child existed is unnamed: {sorted(causes)}",
        )
        self.assertIn("teardown (silent)", causes)
        self.assertIn("timer startup", stderr)

    def test_the_report_lists_the_stage_timeline_in_order(self):
        _, stderr, _, _ = run_timer(self.FULL_RUN)
        self.assertIn("stage timeline, in order", stderr)
        self.assertIn("timer startup", stderr)
        # Compare against the timeline block only. The word "tests" also appears
        # in later sections of the report, so searching the whole output would
        # find the wrong occurrence and the ordering check would be meaningless.
        timeline = stderr.split("stage timeline, in order", 1)[1].split("silent gaps", 1)[0]
        for earlier, later in (
            ("lock-wait", "fetch-index"),
            ("fetch-index", "download"),
            ("download", "fetch-lock"),
            ("fetch-lock", "resolve"),
            ("resolve", "compile"),
            ("compile", "fresh"),
            ("fresh", "build-done"),
            ("build-done", "tests"),
        ):
            self.assertIn(earlier, timeline, f"{earlier} missing from the timeline")
            self.assertIn(later, timeline, f"{later} missing from the timeline")
            self.assertLess(
                timeline.index(earlier),
                timeline.index(later),
                f"{earlier} must be listed before {later}",
            )

    def test_a_run_with_no_build_still_reports_its_stages(self):
        _, stderr, report, _ = run_timer('echo "  Summary [ 0.100s] 0 tests run: 0 passed"')
        self.assertIn("stage timeline, in order", stderr)
        self.assertIn("summary", [p["name"] for p in report["phases"]])
        self.assertLess(report["residual_seconds"], 0.5)


class ArgumentHandling(unittest.TestCase):
    """The child's own flags are never eaten by the timer's parser."""

    def test_flags_after_the_separator_reach_the_child(self):
        # A Makefile recipe can legitimately pass `--json` onward; the timer
        # must not claim it for itself once `--` has been seen.
        # `/bin/sh -c SCRIPT a b c` puts `a` in $0 and the rest in $@, so print
        # both to prove every argument crossed the separator intact.
        code, _, report, _ = run_timer(
            'printf "0=%s\\n" "$0"; printf "args=%s\\n" "$*"',
            args=("--json", "not-ours", "--log", "also-not-ours"),
        )
        self.assertEqual(0, code)
        printed = " | ".join(line["text"] for line in report["all_lines"])
        self.assertIn("0=--json", printed)
        self.assertIn("args=not-ours --log also-not-ours", printed)

    def test_missing_separator_is_a_usage_error(self):
        done = subprocess.run(  # noqa: S603
            [sys.executable, str(TIMER), "make", "test"], capture_output=True, text=True, check=False
        )
        self.assertEqual(2, done.returncode)
        self.assertIn("usage:", done.stderr)

    def test_unknown_flag_is_a_usage_error(self):
        done = subprocess.run(  # noqa: S603
            [sys.executable, str(TIMER), "--nope", "--", "true"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(2, done.returncode)


class Arithmetic(unittest.TestCase):
    """The report's own maths, checked directly rather than through a subprocess."""

    def test_human_renders_each_magnitude(self):
        self.assertEqual("250ms", human(0.25))
        self.assertEqual("1.5s", human(1.5))
        self.assertEqual("2m05s", human(125))
        self.assertEqual("1h01m01s", human(3661))

    def test_residual_is_zero_when_every_span_is_attributed(self):
        report = Report(started=0.0, ended=10.0)
        report.add_gap("tests (silent)", 4.0, 5.0)
        report.open_phase("tests", 0.0).end = 6.0
        self.assertEqual(10.0, report.wall)
        self.assertEqual(0.0, round(report.residual, 6))

    def test_unattributed_time_shows_up_as_residual_not_a_guess(self):
        report = Report(started=0.0, ended=10.0)
        self.assertEqual(10.0, round(report.residual, 6))

    def test_overlapping_phases_are_not_double_counted(self):
        report = Report(started=0.0, ended=10.0)
        report.open_phase("a", 0.0).end = 6.0
        report.open_phase("b", 2.0).end = 8.0
        self.assertEqual(8.0, report.attributed())
        self.assertEqual(2.0, round(report.residual, 6))

    def test_percentiles_are_taken_over_every_test(self):
        report = Report(started=0.0, ended=1.0)
        from phase_report import TestResult

        for index in range(1, 101):
            report.tests.append(TestResult(f"b t{index}", float(index), "PASS", 0.0))
        self.assertEqual(50.0, report.percentile(0.50))
        self.assertEqual(90.0, report.percentile(0.90))
        self.assertEqual(1.0, report.percentile(0.0))

    def test_coverage_gap_is_none_when_nextest_never_reported_a_total(self):
        self.assertIsNone(Report().coverage_gap())


if __name__ == "__main__":
    unittest.main(verbosity=2)
