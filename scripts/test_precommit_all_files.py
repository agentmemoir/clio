#!/usr/bin/env python3
"""Tests for `precommit.py --commit --all-files`, the run behind `make precommit`.

Run: python3 scripts/test_precommit_all_files.py

`make precommit` runs the commit-scoped hooks over every tracked file. `--commit`
alone reads the staged files, which is right for a git hook and empty for a
person asking for a run, so `--all-files` widens the file list and nothing else.
Each case builds a throwaway repository with `test_precommit.py`'s fixtures and
runs the real runner in it. The failure these pin is the quiet one: a hook that
was handed a short list, or no list, and went green.
"""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from precommit_testkit import RepoCase, run, write_hook  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
SHOW = 'echo "mode=$CLIO_PRECOMMIT_MODE files=$*"'
MANUAL = "# precommit-scope: manual\n"


class CommitAllFiles(RepoCase):
    def hooks(self) -> Path:
        directory = self.root / "hooks.d"
        directory.mkdir(exist_ok=True)
        return directory

    def stage_a_change(self) -> None:
        (self.root / "a.rs").write_text("fn main() { }\n", encoding="utf-8")
        subprocess.run(["git", "add", "a.rs"], cwd=self.root, check=True)  # noqa: S603, S607

    def test_every_tracked_text_file_reaches_a_commit_hook(self) -> None:
        write_hook(self.hooks() / "10-show.sh", SHOW)
        self.stage_a_change()
        done = run(self.root, "--commit", "--all-files")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("mode=all files=a.rs b.md sub/d.rs", done.stdout)
        self.assertIn("1 hook(s) over 3 tracked file(s)", done.stdout)

    def test_a_commit_without_the_option_still_reads_only_the_staged_files(self) -> None:
        write_hook(self.hooks() / "10-show.sh", SHOW)
        self.stage_a_change()
        done = run(self.root, "--commit")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("mode=staged files=a.rs\n", done.stdout)
        self.assertIn("1 staged file(s)", done.stdout)

    def test_a_manual_hook_is_left_out_and_said_so(self) -> None:
        write_hook(self.hooks() / "10-show.sh", SHOW)
        write_hook(self.hooks() / "20-slow.sh", MANUAL + "echo ran-the-slow-one; exit 1")
        done = run(self.root, "--commit", "--all-files")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertNotIn("ran-the-slow-one", done.stdout + done.stderr)
        self.assertIn("not commit-scoped, so not run here: 20-slow", done.stdout)

    def test_a_failing_commit_hook_fails_the_run(self) -> None:
        write_hook(self.hooks() / "10-bad.sh", "echo found a leak; exit 1")
        done = run(self.root, "--commit", "--all-files")
        self.assertEqual(1, done.returncode, done.stdout + done.stderr)
        self.assertIn("10-bad: FAIL", done.stderr)

    def test_the_option_without_a_commit_run_is_a_usage_error(self) -> None:
        write_hook(self.hooks() / "10-show.sh", SHOW)
        for flags in (["--all-files"], ["--staged", "--commit", "--all-files"], ["--staged", "--all-files"]):
            done = run(self.root, *flags)
            self.assertEqual(2, done.returncode, f"{flags}: {done.stdout}{done.stderr}")
            self.assertIn("--all-files", done.stderr)
            self.assertNotIn("hook(s) over", done.stdout, "a refused run must not run hooks")


class MakePrecommit(unittest.TestCase):
    def test_the_target_runs_the_commit_scoped_hooks_over_every_tracked_file(self) -> None:
        done = subprocess.run(  # noqa: S603, S607
            ["make", "-n", "precommit"], cwd=ROOT, capture_output=True, text=True, check=False
        )
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("scripts/precommit.py --commit --all-files", done.stdout)


if __name__ == "__main__":
    unittest.main()
