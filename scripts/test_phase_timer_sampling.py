#!/usr/bin/env python3
"""Tests for the sampler and the host capability probe in scripts/phase_timer.py.

Run through the suite entry point: python3 scripts/test_phase_timer.py imports
these classes, so one command runs the whole phase-timer suite.

Why these exist
---------------
The sampler reads the kernel counters of the whole process tree, and it decides
which `ps` keywords this host can answer by running a probe. Both read the
machine, so both are easy to get quietly wrong: a counter that is never read
looks like a run that used no CPU, and a keyword kept for the wrong reason fails
every combined read. A probe that cannot be run at all must not be read as a
directory that refuses execution (issue #60). These tests pin all three.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from phase_timer_testkit import run_timer

import exec_tempdir
from exec_tempdir import exec_tempdir as new_exec_tempdir
from phase_report import Report


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
    and finding it still runs proves nothing. Each candidate is tested with
    `refuses_execution`, which answers True only for a permission refusal from the
    kernel. A probe that could not be run — a transient failure under load —
    answers False, so that candidate is passed over and the test skips rather than
    being handed a directory that in fact permits execution.

    Do not go back to `not can_execute(probe_dir)`. That reads a probe that failed
    for a transient reason as a directory that refuses execution, which is the
    defect issue #60 records: the test then fails instead of skipping. Only
    `refuses_execution` answers the question this helper asks.
    """
    for parent in exec_tempdir.CANDIDATE_PARENTS:
        base = os.path.expanduser(parent) if parent is not None else tempfile.gettempdir()
        try:
            with tempfile.TemporaryDirectory(dir=base) as probe_dir:
                if exec_tempdir.refuses_execution(probe_dir):
                    return base
        except OSError:
            continue
    return None


class FindsWhereExecutionIsRefused(unittest.TestCase):
    """The search for a directory that refuses execution skips a probe that failed.

    This is the end-to-end guard for issue #60. The reported failure was a probe
    that timed out under load and was then read as a directory that refuses
    execution, so the phase-timer test failed instead of skipping.
    """

    def test_a_probe_that_times_out_makes_the_search_report_nothing(self):
        # Every candidate's probe times out. A timeout proves nothing about a
        # directory, so the search must pass each one over and report None, which
        # is what makes the dependent test skip.
        with mock.patch(
            "exec_tempdir.subprocess.run",
            side_effect=subprocess.TimeoutExpired("_probe.sh", 10),
        ):
            self.assertIsNone(
                a_parent_that_refuses_execution(),
                "a timed-out probe must never be returned as a refusing parent",
            )


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


if __name__ == "__main__":
    unittest.main(verbosity=2)
