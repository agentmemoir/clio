from __future__ import annotations

"""The run log: one line per gate run or replay, with its worktree and branch."""

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from exec_tempdir import exec_tempdir
from gate import run_gate
from gate_runlog import RUNS_FILE
from gate_tree import TreeState, cache_root

FIELDS = {"root", "common_dir", "branch", "kind", "started", "finished", "exit", "argv", "replayed"}
TREE = TreeState("h1", (("crates/a/src/lib.rs", "one"),), ("crates/a/src/lib.rs",))


def git(root: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)


def make_repo(parent: Path, branch: str) -> Path:
    """A real repository on `branch` with one commit, configured to answer for itself."""
    root = parent / "main"
    root.mkdir()
    git(root, "init", "-q", "-b", branch)
    for key, value in (("user.name", "t"), ("user.email", "t@e"), ("commit.gpgsign", "false")):
        git(root, "config", "--local", key, value)
    (root / "f").write_text("x\n")
    git(root, "add", "f")
    git(root, "commit", "-q", "-m", "init")
    return root


def run_in(root: Path, cache: Path, argv: list[str]) -> int:
    quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
    with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache)}), mock.patch(
        "gate.repo_root", return_value=str(root)
    ), mock.patch("gate.read_tree_state", return_value=TREE), quiet[0], quiet[1]:
        return run_gate(argv)


def lines(cache: Path) -> list[dict]:
    with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache)}):
        text = (cache_root() / RUNS_FILE).read_text()
    return [json.loads(line) for line in text.splitlines()]


class RunLogTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name).resolve()
        self.cache = self.tmp / "cache"

    def test_a_run_and_a_replay_each_leave_one_line_with_every_field(self) -> None:
        root = make_repo(self.tmp, "feature/x")
        argv = ["/bin/sh", "-c", "exit 7"]
        self.assertEqual(run_in(root, self.cache, argv), 7)
        self.assertEqual(run_in(root, self.cache, argv), 7)
        fresh, replay = lines(self.cache)
        self.assertEqual(set(fresh), FIELDS)
        self.assertEqual(set(replay), FIELDS)
        self.assertEqual((fresh["replayed"], replay["replayed"]), (False, True))
        self.assertEqual((fresh["exit"], replay["exit"]), (7, 7))
        self.assertEqual(fresh["root"], str(root))
        self.assertEqual(fresh["common_dir"], str(root / ".git"))
        self.assertEqual(fresh["branch"], "feature/x")
        self.assertEqual(fresh["argv"], argv)
        self.assertLessEqual(fresh["started"], fresh["finished"])
        self.assertLessEqual(replay["started"], replay["finished"])

    def test_two_worktrees_of_one_repository_share_a_common_dir(self) -> None:
        main = make_repo(self.tmp, "main")
        other = self.tmp / "other"
        git(main, "worktree", "add", "-q", "-b", "feature/y", str(other))
        argv = ["/bin/sh", "-c", "exit 0"]
        run_in(main, self.cache, argv)
        run_in(other, self.cache, argv)
        first, second = lines(self.cache)
        self.assertNotEqual(first["root"], second["root"])
        self.assertEqual(first["common_dir"], second["common_dir"])
        self.assertEqual((first["branch"], second["branch"]), ("main", "feature/y"))

    def test_a_folder_that_is_not_a_repository_logs_nulls_not_a_failure(self) -> None:
        nowhere = self.tmp / "plain"
        nowhere.mkdir()
        self.assertEqual(run_in(nowhere, self.cache, ["/bin/sh", "-c", "exit 0"]), 0)
        (line,) = lines(self.cache)
        self.assertIsNone(line["common_dir"])
        self.assertIsNone(line["branch"])

    def test_a_log_that_cannot_be_written_warns_and_keeps_the_exit_code(self) -> None:
        root = make_repo(self.tmp, "main")
        # The cache works, so the gate runs; only the history file cannot be opened.
        (self.cache / "clio-gate" / RUNS_FILE).mkdir(parents=True)
        err = io.StringIO()
        with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(self.cache)}), mock.patch(
            "gate.repo_root", return_value=str(root)
        ), mock.patch("gate.read_tree_state", return_value=TREE), contextlib.redirect_stdout(
            io.StringIO()
        ), contextlib.redirect_stderr(err):
            code = run_gate(["/bin/sh", "-c", "exit 3"])
        self.assertEqual(code, 3)
        self.assertIn("could not append to the run log", err.getvalue())


class TimedTargetHandoffTests(unittest.TestCase):
    """`make test-fast`, `precommit` and `quick` go to the machine-local hook, and only they do."""

    ROOT = Path(__file__).resolve().parent.parent

    def run_make(self, args: list[str], hook: bool = True, **extra: str) -> tuple[list[str], list[str]]:
        """Run the shim as `make`; return the hook's calls and the real make's calls."""
        with exec_tempdir(purpose="a stand-in make and hook") as raw:
            tmp = Path(raw)
            shimdir, realdir = tmp / "shim", tmp / "real"
            shimdir.mkdir()
            realdir.mkdir()
            real_marker, hook_marker = tmp / "real-calls", tmp / "hook-calls"
            stand_in = realdir / "make"
            stand_in.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{real_marker}"\nexit 0\n')
            stand_in.chmod(0o755)
            (shimdir / "make").symlink_to(self.ROOT / "scripts" / "gate-shim")
            home = tmp / "home"
            if hook:
                (home / ".config" / "clio").mkdir(parents=True)
                script = home / ".config" / "clio" / "gate-hook"
                script.write_text(f'#!/bin/sh\nshift 2\nprintf "%s\\n" "$*" >> "{hook_marker}"\nexit 0\n')
                script.chmod(0o755)
            env = dict(
                os.environ,
                HOME=str(home),
                PATH=f"{shimdir}{os.pathsep}{realdir}{os.pathsep}{os.environ.get('PATH', '')}",
                XDG_CACHE_HOME=str(tmp / "cache"),
            )
            env.update({"GATE_ACTIVE": "", "GATE_RAW": "", "MAKEFLAGS": "", **extra})
            subprocess.run([str(shimdir / "make"), *args], cwd=self.ROOT, env=env, capture_output=True, check=False)

            def calls(path: Path) -> list[str]:
                return path.read_text().splitlines() if path.is_file() else []

            return calls(hook_marker), calls(real_marker)

    def test_the_three_targets_go_to_the_hook_and_not_to_make(self) -> None:
        for target in ("test-fast", "precommit", "quick"):
            hooked, real = self.run_make([target])
            self.assertEqual((hooked, real), ([target], []), target)

    def test_arguments_and_options_travel_with_the_target(self) -> None:
        hooked, real = self.run_make(["quick", "BASE=origin/master"])
        self.assertEqual((hooked, real), (["quick BASE=origin/master"], []))

    def test_other_targets_and_the_bypasses_never_reach_the_hook(self) -> None:
        cases = [(["help"], {}), (["-n", "quick"], {}), (["quick"], {"GATE_RAW": "1"}), (["quick"], {"GATE_ACTIVE": "1"})]
        for args, extra in cases:
            hooked, real = self.run_make(args, **extra)
            self.assertEqual(hooked, [], f"{args} {extra}")
            self.assertEqual(real, [" ".join(args)], f"{args} {extra}")

    def test_without_a_hook_the_target_runs_unrecorded(self) -> None:
        hooked, real = self.run_make(["quick"], hook=False)
        self.assertEqual((hooked, real), ([], ["quick"]))


if __name__ == "__main__":
    unittest.main()
