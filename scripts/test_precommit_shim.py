#!/usr/bin/env python3
"""Tests for the git hook shims that call scripts/precommit.py.

The shims are `scripts/git-hooks/pre-commit` and `pre-merge-commit`. Fixtures are in
`precommit_testkit.py`; run: python3 scripts/test_precommit_shim.py
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from precommit_testkit import LEAK_HOOK, LEAK_MARKER, MERGE_SHIM, RUNNER, RepoCase, SHIM, write_hook  # noqa: E402


class TheCommitHookScript(RepoCase):
    """The shell shim a machine installs as .git/hooks/pre-commit."""

    def install(self, merge: bool = True) -> None:
        """Copy the runner and the shims in, and link them up as git wants them.

        `merge` controls the pre-merge-commit link, because the point of the
        tests that pass False is to show what git does without it.
        """
        (self.root / "scripts" / "git-hooks").mkdir(parents=True)
        for name in ("precommit.py", "precommit_hooks.py"):
            (self.root / "scripts" / name).write_text(
                (RUNNER.parent / name).read_text(encoding="utf-8"), encoding="utf-8"
            )
        shims = [("pre-commit", SHIM)]
        if merge:
            shims.append(("pre-merge-commit", MERGE_SHIM))
        (self.root / ".git" / "hooks").mkdir(parents=True, exist_ok=True)
        for name, source in shims:
            shim = self.root / "scripts" / "git-hooks" / name
            shim.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
            shim.chmod(0o755)
            (self.root / ".git" / "hooks" / name).symlink_to(
                Path(f"../../scripts/git-hooks/{name}")
            )

    def git(self, *args: str) -> subprocess.CompletedProcess:
        # The shim starts the runner, which now imports a sibling module, and
        # Python would write that module's bytecode into the fixture's `scripts/`.
        # The fixture has no ignore file, so `git add -A` would track the .pyc on
        # one branch and a merge would find it untracked on the other.
        return subprocess.run(  # noqa: S603
            ["git", *args],  # noqa: S607
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )

    def commit(self, *args: str) -> subprocess.CompletedProcess:
        return self.git("commit", *args)

    def history(self) -> list[str]:
        done = self.git("log", "--oneline")
        return done.stdout.splitlines()

    def leak_hook(self) -> None:
        """A commit-scoped check that rejects any file containing LEAK."""
        (self.root / "hooks.d").mkdir(exist_ok=True)
        write_hook(self.root / "hooks.d" / "10-leak", body=LEAK_HOOK)

    def commit_all(self, message: str) -> None:
        self.git("add", "-A")
        self.commit("-qm", message)

    def diverge(self, other: str, leak: bool) -> str:
        """Split two branches so a merge has to create a commit.

        Without a commit on the branch being merged into, `git merge` fast-forwards
        and creates nothing, so there is no merge commit for any hook to guard.
        A merge test that quietly fast-forwarded would pass for the wrong reason.
        """
        self.commit_all("base")
        self.git("checkout", "-q", "-b", other)
        (self.root / f"{other}.txt").write_text(f"// {LEAK_MARKER}\n" if leak else "ok\n", encoding="utf-8")
        self.git("add", "-A")
        self.commit("-qm", other, *(["--no-verify"] if leak else []))
        self.git("checkout", "-q", "-")
        (self.root / "here.txt").write_text("here\n", encoding="utf-8")
        self.commit_all("here")
        return other

    def test_the_shim_refuses_a_commit_a_check_rejects(self):
        # The whole point of the shim, proved through the real git: a commit
        # that stages something the check rejects does not happen.
        self.install()
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-leak", body=LEAK_HOOK)
        (self.root / "new.rs").write_text(f"// {LEAK_MARKER}\n", encoding="utf-8")
        subprocess.run(  # noqa: S603
            ["git", "add", "new.rs"], cwd=self.root, check=True  # noqa: S607
        )
        done = self.commit("-m", "should not land")
        self.assertNotEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("found a leak", done.stdout + done.stderr)
        self.assertEqual(
            1,
            len(subprocess.run(  # noqa: S603
                ["git", "log", "--oneline"], cwd=self.root, capture_output=True, text=True, check=False  # noqa: S607
            ).stdout.splitlines()),
            "only the initial commit should exist",
        )

    def test_the_shim_lets_a_clean_commit_through(self):
        self.install()
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-leak", body=LEAK_HOOK)
        (self.root / "new.rs").write_text("fn new() {}\n", encoding="utf-8")
        subprocess.run(  # noqa: S603
            ["git", "add", "new.rs"], cwd=self.root, check=True  # noqa: S607
        )
        done = self.commit("-m", "should land")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)

    def test_the_shim_leaves_a_slow_check_out_of_the_commit(self):
        # A test suite on every commit is a commit hook that gets passed with
        # --no-verify, so the scope declaration has to reach the real git path.
        self.install()
        (self.root / "hooks.d").mkdir()
        (self.root / "hooks.d" / "20-slow").write_text(
            f"#!/bin/sh\n# precommit-scope: manual\nexit 1\n", encoding="utf-8"
        )
        (self.root / "hooks.d" / "20-slow").chmod(0o755)
        (self.root / "new.rs").write_text("fn new() {}\n", encoding="utf-8")
        subprocess.run(  # noqa: S603
            ["git", "add", "new.rs"], cwd=self.root, check=True  # noqa: S607
        )
        done = self.commit("-m", "should land")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)

    def test_the_shim_fails_when_the_runner_is_missing(self):
        # Staying quiet when the runner is gone would be the one state this
        # hook exists to prevent: a commit check reporting success having run
        # nothing at all.
        self.install()
        (self.root / "scripts" / "precommit.py").unlink()
        done = self.commit("-m", "should not land")
        self.assertNotEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("is missing", done.stdout + done.stderr)

    def test_the_shim_works_from_a_subdirectory(self):
        # git runs the hook from the top of the tree, but a person runs it by
        # hand from wherever they are, and it should answer the same either way.
        self.install()
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-leak", body=LEAK_HOOK)
        (self.root / "sub").mkdir(exist_ok=True)
        done = subprocess.run(  # noqa: S603
            [str(self.root / ".git" / "hooks" / "pre-commit")],
            cwd=self.root / "sub",
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("10-leak: pass", done.stdout)

    def test_the_shim_fails_outside_a_repository(self):
        self.install()
        with tempfile.TemporaryDirectory() as bare:
            done = subprocess.run(  # noqa: S603
                [str(self.root / ".git" / "hooks" / "pre-commit")],
                cwd=bare,
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertNotEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("not inside a git repository", done.stdout + done.stderr)

    def test_the_merge_shim_refuses_a_merge_that_carries_a_leak(self):
        # git does not consult pre-commit for a merge that completes on its own,
        # and the pipeline brings the integration branch in with `git merge`, so
        # without this hook the most common commit in the system is unchecked.
        self.install()
        self.leak_hook()
        self.diverge("side", leak=True)
        done = self.git("merge", "side", "-m", "merge")
        self.assertNotEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("found a leak", done.stdout + done.stderr)
        self.assertNotIn("merge", [line.split(" ", 1)[-1] for line in self.history()])

    def test_the_merge_shim_lets_a_clean_merge_through(self):
        # A hook that refuses every merge is not a gate, it is an outage. This is
        # the case that keeps the merge hook honest.
        self.install()
        self.leak_hook()
        self.diverge("side", leak=False)
        done = self.git("merge", "side", "-m", "merge")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertEqual(
            2,
            len(self.git("log", "-1", "--format=%p").stdout.split()),
            "this should be a real merge commit, not a fast-forward",
        )

    def test_without_the_merge_shim_a_merge_really_is_unchecked(self):
        # The control for the two tests above. If this ever starts failing, the
        # leak hook is no longer rejecting files and the merge tests prove
        # nothing; it is the assertion that keeps them meaningful.
        self.install(merge=False)
        self.leak_hook()
        self.diverge("side", leak=True)
        done = self.git("merge", "side", "-m", "merge")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertNotIn("precommit:", done.stdout + done.stderr)

    def test_a_resolved_conflict_still_goes_through_pre_commit(self):
        # git documents that pre-merge-commit does not run for a merge needing
        # resolution, and that pre-commit runs when the result is committed. The
        # pipeline resolves conflicts and commits, so the leak has to be caught
        # on that second step rather than assumed caught on the first.
        self.install()
        self.leak_hook()
        self.commit_all("base")
        start = self.git("branch", "--show-current").stdout.strip()
        # Both branches from the same commit: cut the second from the recorded
        # start point, not from the first branch, or the second is simply the
        # first plus a change and the merge fast-forwards with nothing to
        # conflict over.
        for name, text in (("one", "one\n"), ("two", "two\n")):
            self.git("checkout", "-q", "-b", name, start)
            (self.root / "both.txt").write_text(text, encoding="utf-8")
            self.git("add", "-A")
            self.git("commit", "-qm", name)
        self.git("checkout", "-q", "one")
        conflicted = self.git("merge", "two", "-m", "c")
        self.assertNotEqual(0, conflicted.returncode, conflicted.stdout)
        self.assertIn("CONFLICT", conflicted.stdout + conflicted.stderr)
        (self.root / "both.txt").write_text(f"// {LEAK_MARKER}\n", encoding="utf-8")
        self.git("add", "both.txt")
        done = self.commit("--no-edit")
        self.assertNotEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("found a leak", done.stdout + done.stderr)

    def test_a_linked_worktree_still_finds_the_machine_local_hooks(self):
        # git shares .git/hooks across worktrees, so this shim runs in a linked
        # worktree, but the machine-local hook directory is registered in the
        # main checkout. Without the common-dir lookup the run finds no
        # commit-scoped check, says so, and passes -- which is the exact state
        # this runner exists to refuse.
        self.install()
        self.leak_hook()
        self.commit_all("base")
        main = self.root
        work = self.root.parent / f"{self.root.name}-wt"
        added = subprocess.run(  # noqa: S603
            ["git", "worktree", "add", "-q", "-b", "wt", str(work)],  # noqa: S607
            cwd=main,
            capture_output=True,
            text=True,
            check=False,
        )
        self.addCleanup(subprocess.run, ["git", "worktree", "remove", "--force", str(work)], cwd=main, check=False)  # noqa: S603, S607
        if added.returncode != 0:
            self.skipTest(f"git worktree add is unavailable here: {added.stderr.strip()}")
        # The machine-local directory lives in the main checkout, as it does on a
        # real machine: the worktree is a second working tree, not a second clone.
        (main / ".clio-hooks").mkdir(exist_ok=True)
        write_hook(main / ".clio-hooks" / "20-local", body=LEAK_HOOK)
        (work / "leak.txt").write_text(f"// {LEAK_MARKER}\n", encoding="utf-8")
        staged = subprocess.run(  # noqa: S603
            ["git", "add", "leak.txt"], cwd=work, capture_output=True, text=True, check=True  # noqa: S607
        )
        self.assertEqual(0, staged.returncode, staged.stderr)
        done = subprocess.run(  # noqa: S603
            ["git", "commit", "-qm", "in a worktree"],  # noqa: S607
            cwd=work,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("20-local", done.stdout + done.stderr)
        self.assertIn("found a leak", done.stdout + done.stderr)

    def test_the_merge_shim_fails_when_the_commit_shim_is_missing(self):
        # It delegates, so a missing delegate must be a failure rather than a
        # merge that quietly skips every check.
        self.install()
        self.leak_hook()
        self.diverge("side", leak=False)
        (self.root / "scripts" / "git-hooks" / "pre-commit").unlink()
        done = self.git("merge", "side", "-m", "merge")
        self.assertNotEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("is missing", done.stdout + done.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
