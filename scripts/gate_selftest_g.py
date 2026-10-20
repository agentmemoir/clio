from __future__ import annotations

"""The Done check at the gate: a program's `make coverage-fast` is refused unless it fits and has not run."""

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import gate_done_check as done
from gate import run_gate
from gate_runlog import RUNS_FILE, machine_load
from gate_selftest_d import TREE, lines, make_repo
from gate_tree import TreeState, cache_root

MAKE = ["/usr/bin/make", "coverage-fast", "BASE=origin/master"]
FITS = ({"done_seconds": 90.0, "within": True, "budget_seconds": 180.0}, "")
OVER = ({"done_seconds": 400.0, "within": False, "budget_seconds": 180.0}, "")
UNKNOWN = ({"done_seconds": None, "within": None, "budget_seconds": 180.0}, "")
OTHER_TREE = TreeState("h2", (("crates/a/src/lib.rs", "two"),), ("crates/a/src/lib.rs",))
THIRD_TREE = TreeState("h3", (("crates/a/src/lib.rs", "three"),), ("crates/a/src/lib.rs",))
FOURTH_TREE = TreeState("h4", (("crates/a/src/lib.rs", "four"),), ("crates/a/src/lib.rs",))


class GateDecisionTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name).resolve()
        self.cache = self.tmp / "cache"
        self.root = make_repo(self.tmp, "feature/x")

    def attempt(self, argv=MAKE, estimate=FITS, terminal=False, tree=TREE, output=b""):
        """One gate run with no real make and a stand-in estimate; (exit code, stderr, estimate asked, make called)."""
        err = io.StringIO()

        def fake_make(*args, stdout, **kwargs):
            stdout.write(output)
            return 0

        with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(self.cache)}), mock.patch(
            "gate.repo_root", return_value=str(self.root)
        ), mock.patch("gate.read_tree_state", return_value=tree), mock.patch.object(
            done, "_estimate", return_value=estimate
        ) as asked, mock.patch.object(done, "_terminal_attached", return_value=terminal), mock.patch(
            "gate.subprocess.call", side_effect=fake_make
        ) as ran, contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            code = run_gate(argv)
        return code, err.getvalue(), asked, ran

    def test_a_run_that_fits_goes_on_and_logs_its_estimate(self) -> None:
        code, _, asked, ran = self.attempt()
        self.assertEqual(code, 0)
        ran.assert_called_once()
        asked.assert_called_once_with(str(self.root), "origin/master")
        (line,) = lines(self.cache)
        self.assertEqual((line["kind"], line["estimate_s"], line["replayed"]), ("coverage-fast", 90.0, False))
        self.assertIsInstance(line["tree"], str)
        self.assertEqual(set(line["load"]), {"avg1", "cpus"})

    def test_a_run_over_the_budget_is_refused_and_nothing_runs(self) -> None:
        code, err, _, ran = self.attempt(estimate=OVER)
        self.assertEqual(code, done.EXIT_REFUSED)
        ran.assert_not_called()
        self.assertIn("gate: coverage-fast refused: it is estimated at 400 s, over the 180 s budget", err)
        self.assertIn("CI decides the coverage verdict", err)
        self.assertFalse((self.cache / "clio-gate" / RUNS_FILE).exists(), "a refusal is not a run")

    def test_a_run_that_cannot_be_estimated_is_refused(self) -> None:
        code, err, _, ran = self.attempt(estimate=UNKNOWN)
        self.assertEqual(code, done.EXIT_REFUSED)
        self.assertIn("its length is unknown", err)
        code, err, _, _ = self.attempt(estimate=(None, "quick: no such ref"))
        self.assertEqual(code, done.EXIT_REFUSED)
        self.assertIn("its length cannot be estimated (quick: no such ref)", err)
        ran.assert_not_called()

    def test_a_fresh_run_records_the_test_seconds_nextest_reported(self) -> None:
        self.assertEqual(self.attempt(output=b"     Summary [   4.932s] 335 tests run: 335 passed, 6616 skipped\n")[0], 0)
        (line,) = lines(self.cache)
        self.assertEqual(line["test_s"], 4.932)

    def test_a_second_fresh_run_on_a_changed_tree_is_allowed(self) -> None:
        self.assertEqual(self.attempt()[0], 0)
        code, _, asked, ran = self.attempt(tree=OTHER_TREE)
        self.assertEqual(code, 0)
        asked.assert_called_once()
        ran.assert_called_once()
        first, second = lines(self.cache)
        self.assertNotEqual(first["tree"], second["tree"])

    def test_the_fourth_fresh_run_is_refused_and_a_stored_tree_still_replays(self) -> None:
        for tree in (TREE, OTHER_TREE, THIRD_TREE):
            self.assertEqual(self.attempt(tree=tree)[0], 0)
        code, err, asked, ran = self.attempt(tree=FOURTH_TREE)
        self.assertEqual(code, done.EXIT_REFUSED)
        self.assertIn("it already ran 3 times on branch feature/x in this worktree, the most allowed", err)
        asked.assert_not_called()
        ran.assert_not_called()
        code, _, asked, ran = self.attempt(tree=TREE)
        self.assertEqual(code, 0)
        asked.assert_not_called()
        ran.assert_not_called()

    def test_a_replay_of_the_stored_verdict_is_allowed(self) -> None:
        self.assertEqual(self.attempt()[0], 0)
        code, _, asked, ran = self.attempt()
        self.assertEqual(code, 0)
        asked.assert_not_called()
        ran.assert_not_called()
        fresh, replay = lines(self.cache)
        self.assertEqual((fresh["replayed"], replay["replayed"], replay["estimate_s"]), (False, True, None))
        self.assertEqual(fresh["tree"], replay["tree"])

    def test_coverage_fast_after_another_goal_is_still_judged(self) -> None:
        code, err, _, ran = self.attempt(argv=["/usr/bin/make", "lint", *MAKE[1:]], estimate=OVER)
        self.assertEqual(code, done.EXIT_REFUSED)
        self.assertIn("gate: coverage-fast refused", err)
        ran.assert_not_called()

    def test_a_person_at_a_terminal_is_never_refused(self) -> None:
        code, _, asked, ran = self.attempt(estimate=OVER, terminal=True)
        self.assertEqual(code, 0)
        asked.assert_not_called()
        ran.assert_called_once()

    def test_a_plan_and_other_targets_are_not_judged(self) -> None:
        with mock.patch.object(done, "_estimate") as asked, mock.patch.object(done, "_terminal_attached", return_value=False):
            for kind, argv in (
                ("coverage-fast", [*MAKE, "PLAN=1"]),
                ("lint", ["/usr/bin/make", "lint"]),
                ("coverage", ["/usr/bin/make", "coverage"]),
            ):
                self.assertEqual(done.decide(kind, argv, str(self.root)), done.ALLOW, kind)
            asked.assert_not_called()


class HelperTests(unittest.TestCase):
    def test_base_comes_from_the_argument_and_defaults_to_the_makefiles_master(self) -> None:
        self.assertEqual(done._base(["make", "coverage-fast"]), "master")
        self.assertEqual(done._base(["make", "coverage-fast", "BASE=origin/master"]), "origin/master")
        self.assertEqual(done._base(["make", "coverage-fast", "BASE="]), "master")

    def test_machine_load_names_the_one_minute_average_and_the_cpu_count(self) -> None:
        with mock.patch.object(os, "getloadavg", return_value=(1.234, 0.5, 0.2)):
            self.assertEqual(machine_load(), {"avg1": 1.23, "cpus": os.cpu_count()})
        with mock.patch.object(os, "getloadavg", side_effect=OSError):
            self.assertIsNone(machine_load()["avg1"])

    def test_only_a_real_earlier_run_on_this_worktree_and_branch_counts(self) -> None:
        with tempfile.TemporaryDirectory() as raw, mock.patch.dict(os.environ, {"XDG_CACHE_HOME": raw}):
            root = str(Path(raw).resolve())
            real = {"kind": "coverage-fast", "replayed": False, "argv": MAKE, "root": root, "branch": "b"}
            misses = [
                {**real, "replayed": True},
                {**real, "argv": [*MAKE, "PLAN=1"]},
                {**real, "root": "/elsewhere"},
                {**real, "branch": "other"},
                {**real, "kind": "lint"},
            ]
            path = cache_root() / RUNS_FILE
            path.parent.mkdir(parents=True)
            path.write_text("\n".join(["not json", *map(json.dumps, misses)]) + "\n")
            self.assertEqual(done._fresh_runs(root, "b"), 0)
            path.write_text(path.read_text() + (json.dumps(real) + "\n") * 2)
            self.assertEqual(done._fresh_runs(root, "b"), 2)

    def test_the_estimate_is_read_from_quick_and_every_failure_is_a_reason(self) -> None:
        def answer(code: int, out: str = "", err: str = "") -> mock._patch:
            return mock.patch.object(done.subprocess, "run", return_value=subprocess.CompletedProcess([], code, out, err))

        with answer(0, '{"within": true}'):
            self.assertEqual(done._estimate("/r", "b"), ({"within": True}, ""))
        with answer(3, "", "quick: boom\n"):
            self.assertEqual(done._estimate("/r", "b"), (None, "quick: boom"))
        with answer(0, "[1]"):
            self.assertEqual(done._estimate("/r", "b"), (None, "unreadable answer"))
        with answer(0, "not json"):
            self.assertIsNone(done._estimate("/r", "b")[0])
        with mock.patch.object(done.subprocess, "run", side_effect=subprocess.TimeoutExpired("quick", 60)):
            self.assertIsNone(done._estimate("/r", "b")[0])


if __name__ == "__main__":
    unittest.main()
