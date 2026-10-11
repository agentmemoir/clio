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


if __name__ == "__main__":
    unittest.main()
