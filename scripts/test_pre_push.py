"""Check pushed bytes, skip cases, and detached worktree cleanup."""

from __future__ import annotations

import os
import shutil
import subprocess
import unittest
from pathlib import Path

from precommit_testkit import LEAK_HOOK, RepoCase, write_hook

SHIM = Path(__file__).resolve().parent / "git-hooks" / "pre-push"
ZERO = "0" * 40


class PushTests(RepoCase):
    def setUp(self) -> None:
        super().setUp()
        self.git("branch", "-M", "master")
        self.base = self.git("rev-parse", "HEAD").stdout.strip()
        self.git("update-ref", "refs/remotes/origin/master", self.base)
        scripts = self.root / "scripts"
        scripts.mkdir()
        for name in ("precommit.py", "precommit_hooks.py"):
            shutil.copy(SHIM.parent.parent / name, scripts / name)
        self.hooks = self.root / ".clio-hooks"
        self.hooks.mkdir()
        write_hook(self.hooks / "10-leak.sh", LEAK_HOOK)
        self.git("add", "scripts")
        self.git("commit", "-qm", "runner")
        self.clean = self.git("rev-parse", "HEAD").stdout.strip()

    def git(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["git", *args], cwd=self.root, check=True, capture_output=True, text=True
        )

    def push(self, sha: str, remote: str | None = None) -> subprocess.CompletedProcess:
        environment = dict(os.environ)
        environment.pop("CLIO_PRECOMMIT_HOOKS", None)
        # Reproduce Git's exported context; it must not leak into the scan tree.
        environment["GIT_DIR"] = str(self.root / ".git")
        environment["GIT_WORK_TREE"] = str(self.root)
        result = subprocess.run(
            ["sh", str(SHIM)],
            cwd=self.root,
            env=environment,
            input=f"refs/heads/master {sha} refs/heads/master {remote or self.base}\n",
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(
            len(self.git("worktree", "list", "--porcelain").stdout.split("worktree "))
            - 1,
            1,
        )
        return result

    def test_clean_tree(self) -> None:
        self.assertEqual(self.push(self.clean).returncode, 0)

    def test_pushed_leak_rejected_even_when_local_file_clean(self) -> None:
        (self.root / "a.rs").write_text("LEAK\n")
        self.git("add", "a.rs")
        self.git("commit", "-qm", "leak")
        sha = self.git("rev-parse", "HEAD").stdout.strip()
        (self.root / "a.rs").write_text("clean\n")
        result = self.push(sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("found a leak", result.stdout + result.stderr)

    def test_whole_tree_includes_unchanged_leak(self) -> None:
        (self.root / "a.rs").write_text("LEAK\n")
        self.git("add", "a.rs")
        self.git("commit", "-qm", "old leak")
        remote = self.git("rev-parse", "HEAD").stdout.strip()
        (self.root / "b.md").write_text("new docs\n")
        self.git("add", "b.md")
        self.git("commit", "-qm", "docs")
        result = self.push(self.git("rev-parse", "HEAD").stdout.strip(), remote)
        self.assertNotEqual(result.returncode, 0)

    def test_new_branch_uses_base(self) -> None:
        self.assertEqual(self.push(self.clean, ZERO).returncode, 0)

    def test_branch_delete_skips(self) -> None:
        write_hook(self.hooks / "20-fail.sh", "exit 1")
        self.assertEqual(self.push(ZERO).returncode, 0)

    def test_deletion_only_skips(self) -> None:
        self.git("rm", "a.rs")
        self.git("commit", "-qm", "delete")
        write_hook(self.hooks / "20-fail.sh", "exit 1")
        sha = self.git("rev-parse", "HEAD").stdout.strip()
        self.assertEqual(self.push(sha, self.clean).returncode, 0)

    def test_git_push_refuses_leak(self) -> None:
        remote = self.root / "remote.git"
        self.git("init", "--bare", str(remote))
        self.git("remote", "add", "origin", str(remote))
        (self.root / ".git" / "hooks" / "pre-push").symlink_to(SHIM)
        (self.root / "a.rs").write_text("LEAK\n")
        self.git("add", "a.rs")
        self.git("commit", "-qm", "leak")
        result = subprocess.run(
            ["git", "push", "origin", "master"],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("found a leak", result.stdout + result.stderr)
        self.assertNotIn("detached", self.git("worktree", "list").stdout)

    def test_main_local_hook_failure_cleans_worktree(self) -> None:
        write_hook(self.hooks / "20-fail.sh", "exit 1")
        self.assertNotEqual(self.push(self.clean).returncode, 0)


if __name__ == "__main__":
    unittest.main()
