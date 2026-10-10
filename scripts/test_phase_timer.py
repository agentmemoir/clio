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
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

import exec_tempdir  # noqa: E402
from exec_tempdir import exec_tempdir as new_exec_tempdir  # noqa: E402
from phase_report import Report, human  # noqa: E402

TIMER = SCRIPTS / "phase_timer.py"

# A child's output shaped exactly like cargo + nextest, so the parsing paths are
# exercised without paying for a real 5-minute build.
FAKE_RUN = r"""
echo "    Blocking waiting for file lock on build directory"
echo "   Compiling clio v0.1.0 (/repo/crates/clio)"
echo "    Finished `test` profile [unoptimized + debuginfo] target(s) in 1m 30s"
echo "  Starting 3 tests across 2 binaries (10 tests skipped)"
echo "        PASS [   1.500s] ( 1/3) clio::bin/clio alpha::one"
echo "        PASS [   0.250s] ( 2/3) clio::bin/clio alpha::two"
echo "        FAIL [   0.750s] ( 3/3) clio-store beta::three"
echo "  Summary [   2.500s] 3 tests run: 2 passed, 1 failed"
exit 100
"""


def run_timer(script: str, *extra: str, args: tuple[str, ...] = ()) -> tuple[int, str, dict, str]:
    """Run the timer over `script`; return exit code, stderr report, JSON, raw log."""
    with tempfile.TemporaryDirectory() as tmp:
        json_path = Path(tmp) / "timing.json"
        log_path = Path(tmp) / "raw.log"
        argv = [
            sys.executable,
            str(TIMER),
            "--json",
            str(json_path),
            "--log",
            str(log_path),
            *extra,
            "--",
            "/bin/sh",
            "-c",
            script,
            *args,
        ]
        done = subprocess.run(argv, capture_output=True, text=True, check=False)  # noqa: S603
        report = json.loads(json_path.read_text()) if json_path.exists() else {}
        raw = log_path.read_text() if log_path.exists() else ""
        return done.returncode, done.stderr, report, raw


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


class MeasuresTheKernelNotJustTheOutput(unittest.TestCase):
    """The report must see what the kernel charged the run, not just its stdout.

    A build can spend most of its time inside the kernel and print an ordinary
    looking log, so a tool that only reads the child's lines cannot see the
    dominant cost. These pin the counter plumbing, using a child that burns real
    CPU and real I/O rather than a mocked counter source.
    """

    #: Burns about a second of CPU in the kernel (page faults) and prints a line.
    BUSY_CHILD = (
        "i=0; while [ $i -c ]; do :; done\n"
        "dd if=/dev/zero of=/dev/null bs=1m count=200 2>/dev/null\n"
        "echo '     PASS [   1.000s] ( 1/1) demo::bin/demo busy::one'\n"
    )

    def test_cpu_time_is_measured_and_split_user_from_system(self):
        _, stderr, report, _ = run_timer(self.BUSY_CHILD)
        self.assertIn("kernel counters", stderr)
        self.assertRegex(stderr, r"cpu\s+: .*user \+ .*system")
        self.assertGreater(report["resources"]["totals"]["system_seconds"], 0.0)

    def test_a_child_that_burns_cpu_is_reported_as_such(self):
        _, stderr, report, _ = run_timer("i=0; while [ $i -c ]; do :; done; echo hi")
        totals = report["resources"]["totals"]
        self.assertGreater(totals["user_seconds"] + totals["system_seconds"], 0.0)
        self.assertTrue("cpu-bound" in stderr or "kernel-bound" in stderr)

    def test_sampling_coverage_is_stated_not_implied(self):
        _, stderr, report, _ = run_timer("sleep 2; echo hi")
        self.assertIn("coverage:", stderr)
        self.assertIn("source:", stderr)
        self.assertTrue(report["resources"]["coverage_note"])
        # The note must quantify how much of the run was actually observed.
        self.assertRegex(report["resources"]["coverage_note"], r"\d+ samples every")

    def test_samples_are_recorded_over_time_not_only_at_the_end(self):
        _, _, report, _ = run_timer("sleep 2; echo hi")
        samples = report["resources"]["samples"]
        self.assertGreaterEqual(len(samples), 2, "a two-second run must yield several samples")
        offsets = [s["at_offset"] for s in samples]
        self.assertEqual(offsets, sorted(offsets))
        self.assertGreater(offsets[-1] - offsets[0], 0.5, "samples must span the run")

    def test_process_count_is_tracked_so_a_short_tree_is_visible(self):
        _, _, report, _ = run_timer("sleep 1; echo hi")
        self.assertTrue(any(s["children"] >= 1 for s in report["resources"]["samples"]))

    def test_page_faults_are_recorded(self):
        _, stderr, report, _ = run_timer("i=0; while [ $i -c ]; do :; done; echo hi")
        totals = report["resources"]["totals"]
        self.assertGreater(totals["minor_faults"], 0.0)
        self.assertIn("faults", stderr)

    def test_a_child_that_exits_at_once_still_reports_counters(self):
        code, stderr, report, _ = run_timer("echo hi")
        self.assertEqual(0, code)
        self.assertIn("kernel counters", stderr)
        self.assertIsNotNone(report["resources"]["totals"])

    def test_unavailable_counters_are_named_rather_than_reported_as_zero(self):
        # A run too short to sample must not read as "this run used no CPU".
        _, stderr, report, _ = run_timer("echo hi")
        note = report["resources"]["coverage_note"]
        self.assertTrue(note)
        if report["resources"]["unavailable_reason"]:
            self.assertIn("note", stderr)


#: A stand-in for a `ps` that refuses two keywords the way procps does — with a
#: message that is not the one macOS prints — and serves every other one. The
#: probe used to keep any keyword whose stderr did not contain "not found", so
#: on such a host it kept both, every combined read then failed, and a
#: two-second run reported that it was too short to have been sampled. Exits
#: non-zero, as the real one does.
#:
#: The refused pair is `rss` and `minflt` rather than the three Linux-only
#: keywords, because those three are already unsupported on macOS: refusing them
#: here would produce the same answer the real `ps` gives, and the test would
#: pass whether or not the probe had been fixed. Refusing a field every host
#: serves is the only way for this to fail on the old probe everywhere.
#: The figure both stubs print for `time`. It belongs here rather than inside each
#: stub body so that changing it once changes the stub and the check that proves
#: the stub ran. A number written in both places would leave the check asserting
#: something the stubs no longer produce, and it would do so silently.
PS_STUB_TIME_KB = "1024"

PS_REFUSING_TWO_KEYWORDS = r"""#!/bin/sh
for arg in "$@"; do
  case "$arg" in
    rss=|minflt=)
      echo "error: unknown user-defined format specifier \"${arg%=}\"" >&2
      exit 1
      ;;
  esac
done
echo "RSS    TIME"
echo "__PS_STUB_TIME__   0:00.01"
exit 0
"""

#: A `ps` that answers every request and warns on the way, in words that include
#: the phrase the old probe searched for. A message-based probe would drop fields
#: that work; only the exit status says whether the read can succeed.
PS_WARNING_BUT_SUCCEEDING = r"""#!/bin/sh
echo "ps: rss: keyword not found in the boot cache, estimating instead" >&2
echo "RSS    TIME"
echo "__PS_STUB_TIME__   0:00.01"
exit 0
"""


def stub_script(template: str) -> str:
    """A stub body with the shared fixture figure filled in."""
    return template.replace("__PS_STUB_TIME__", PS_STUB_TIME_KB)


def a_parent_that_refuses_execution() -> str | None:
    """A writable parent whose subdirectories refuse to run an executable file.

    Found by asking rather than assumed, because most hosts have a usable temporary
    directory and this branch of the fix would then never be exercised anywhere.
    `None` means this host has nowhere that refuses, and the test that wants one
    skips rather than asserting something it cannot set up.

    A mount that refuses execution has to be found, not invented: writing a probe
    and finding it still runs proves nothing. Each candidate is tested by asking
    whether a directory inside it refuses.
    """
    for parent in exec_tempdir.CANDIDATE_PARENTS:
        base = os.path.expanduser(parent) if parent is not None else tempfile.gettempdir()
        try:
            with tempfile.TemporaryDirectory(dir=base) as probe_dir:
                if not exec_tempdir.can_execute(probe_dir):
                    return base
        except OSError:
            continue
    return None


class RecognisesWhatTheHostSupports(unittest.TestCase):
    """The keyword probe reads the exit status, not the wording of a message.

    `ps` refuses an unknown keyword in a different sentence on every platform:
    macOS says `readb: keyword not found`, procps says `unknown user-defined
    format specifier "readb"`. Matching the text marked a keyword supported on a
    host that cannot print it, and one unreadable field then failed every read,
    so the sampler silently recorded nothing for the whole run.

    Both cases here are driven by a stub, and a stub that cannot run makes these
    tests worse than useless: the host's real `ps` answers instead, so they report
    the host's own field list as if it were the probe's, and a probe that had gone
    back to matching messages could still pass. `_probe_with_stubbed_ps` therefore
    builds the stub somewhere it runs and proves it ran.
    """

    def _probe_with_stubbed_ps(
        self,
        stub: str,
        parents: tuple[str | None, ...] | None = None,
    ) -> list[str]:
        """Run the probe with `stub` standing in for `ps` earlier on PATH.

        The stub is built in a directory the host permits running an executable
        file in, and is then run once by hand. Both steps matter. A stub written
        into a directory mounted `noexec` is skipped rather than refused:
        `execvp` reads a permission failure on one PATH entry as "this one is
        unusable, keep looking" and carries on down the list, so the host's real
        `ps` answers and this helper measures the host instead of the probe. The
        field list it returns then looks like a real answer, and the failure points
        at a list of fields rather than at a mount.

        So the directory is chosen for executability first, and the stub is proved
        to run before the probe is asked anything. Without that second step a
        regression this class exists to catch — a probe reading the wording of
        `ps`'s message instead of its exit status — could pass here, because the
        stub producing that wording never ran.

        `parents` exists so a test can point the helper at a directory that refuses
        execution, which is the only way to reach the refusal path on a host whose
        temporary directory is in fact usable.
        """
        import phase_timer

        with new_exec_tempdir(purpose="the stub ps", parents=parents) as tmp:
            fake = Path(tmp) / "ps"
            fake.write_text(stub_script(stub))
            fake.chmod(0o755)
            ahead = os.pathsep.join([str(fake.parent), *os.environ["PATH"].split(os.pathsep)])
            saved_path, os.environ["PATH"] = os.environ["PATH"], ahead
            saved_fields, phase_timer._PS_FIELDS = phase_timer._PS_FIELDS, None
            try:
                self._assert_the_stub_is_what_answers()
                return phase_timer._supported_ps_fields()
            finally:
                os.environ["PATH"] = saved_path
                phase_timer._PS_FIELDS = saved_fields

    def _assert_the_stub_is_what_answers(self) -> None:
        """Require the stub on PATH to answer as itself, before the probe is asked.

        Run by name rather than by path, because by name is how the probe reaches
        it: `execvp` walks PATH, and a check that invoked the file directly would
        pass in exactly the case that matters, where the entry is skipped for
        being unusable and the real `ps` answers in its place.

        Both stubs print the shared fixture figure for `time`, which is what tells
        them apart from a real `ps` reporting this process's own value. The number
        is the stub's, not the host's, so nothing here depends on the machine.

        Failing here is the point. A stub that did not run means every assertion
        downstream is measuring the host, and a check that cannot fail is what let
        the false failure through in the first place.
        """
        try:
            done = subprocess.run(  # noqa: S603
                ["ps", "-o", "time=", "-p", str(os.getpid())],
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as refused:
            self.fail(f"the stub could not be run at all ({refused}), so the probe would measure the host's real ps")
        # The stubs print a header row and then their own figure, so the figure is
        # the second line's first field. A real `ps` answers `time` in seconds and
        # prints no such header, which is what makes this a check of the stub rather
        # than a restatement of the two tests' own assertions.
        rows = [line.split() for line in done.stdout.splitlines() if line.split()]
        answered = rows[1][0] if len(rows) > 1 else ""
        self.assertEqual(
            PS_STUB_TIME_KB,
            answered,
            f"the stub did not answer as itself, so the probe would have measured "
            f"the host's real ps (stdout={done.stdout!r} stderr={done.stderr!r})",
        )

    def test_a_host_whose_stub_cannot_run_is_caught_rather_than_measured(self):
        # The failure this check exists to prevent is silent: a stub that does not
        # run lets the real `ps` answer, and the assertion then describes the host.
        # A probe that had gone back to matching the wording of `ps`'s message would
        # pass on such a host. So the guard is driven here by a directory that
        # refuses execution while the executability check is told the directory is
        # fine — the only way to reach the guard from a host whose temporary
        # directory is in fact usable.
        refusing = a_parent_that_refuses_execution()
        if refusing is None:
            self.skipTest("no candidate directory on this host refuses to run an executable file")
        # The executability check is told the directory is fine, so the helper
        # hands back one that refuses. That is the only way to reach the guard
        # from a host whose temporary directory is in fact usable, and it is what
        # makes this a test of the guard rather than of the host.
        asked: list[str] = []
        original = exec_tempdir.can_execute
        exec_tempdir.can_execute = lambda directory: asked.append(directory) or True
        self.addCleanup(setattr, exec_tempdir, "can_execute", original)
        with self.assertRaises(AssertionError) as caught:
            self._probe_with_stubbed_ps(PS_REFUSING_TWO_KEYWORDS, parents=(refusing,))
        message = str(caught.exception)
        self.assertTrue(asked, "the executability check must be the one that was replaced")
        self.assertIn("did not answer as itself", message, message)
        # What answered in the stub's place is the host's own `ps`, which reports
        # a duration and prints no header row. Saying so is the whole value of
        # failing here: it is the difference between a reader being told the
        # directory refused and being handed a field list to misread.
        self.assertIn("measured the host's real ps", message, message)

    def test_a_refused_keyword_is_dropped_whatever_the_message_says(self):
        supported = self._probe_with_stubbed_ps(PS_REFUSING_TWO_KEYWORDS)
        for refused in ("rss", "minflt"):
            self.assertNotIn(
                refused,
                supported,
                f"{refused} was refused with a non-zero exit and must not be requested",
            )
        for kept in ("time", "majflt", "nvcsw", "nivcsw", "readb", "writeb", "rsspeak"):
            self.assertIn(kept, supported, f"{kept} was accepted and must be requested")

    def test_a_keyword_answered_despite_a_warning_is_still_kept(self):
        supported = self._probe_with_stubbed_ps(PS_WARNING_BUT_SUCCEEDING)
        self.assertEqual(
            len(supported),
            9,
            f"every keyword exited zero, so all are kept; got {supported}",
        )

    def test_a_sampler_that_reads_nothing_says_so_instead_of_blaming_the_run_length(self):
        # The coverage note used to say "the run was shorter than one sampling
        # interval" for every zero-sample run, including one where every read
        # failed. That is a claim about the run's length and it is false when the
        # reader is broken, so a reader failure is recorded as an error instead.
        import phase_timer
        from phase_report import Report

        report = Report(started=time.time())
        saved = phase_timer._read_counters
        phase_timer._read_counters = lambda _pid: None
        try:
            phase_timer._sample_tree(report, os.getpid())
        finally:
            phase_timer._read_counters = saved

        self.assertEqual(report.resources.samples, [], "nothing was read, so nothing sampled")
        self.assertTrue(
            report.resources.errors,
            "processes were found and none could be read, which must be reported as a failure",
        )
        self.assertNotIn(
            "shorter than one sampling",
            report.resources.coverage_note(10.0),
            "a broken reader must not be reported as a run that was too short",
        )


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
