#!/usr/bin/env python3
"""Tests for scripts/precommit.py: which files a hook is given, where it runs, and the
commit and manual scopes.

Fixtures are in `precommit_testkit.py`; run: python3 scripts/test_precommit_scopes.py
"""

from __future__ import annotations

import os
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from precommit_testkit import LEAK_HOOK, LEAK_MARKER, RepoCase, run, write_hook  # noqa: E402


class ChoosesFiles(RepoCase):
    """The file list is tracked text, and nothing else."""

    def test_binary_files_are_left_out(self):
        (self.root / "hooks.d").mkdir()
        write_hook(
            self.root / "hooks.d" / "10-count",
            body="echo \"count=$#\"; exit 0",
        )
        done = run(self.root)
        self.assertIn("count=3", done.stdout, "a.rs, b.md, sub/d.rs only")

    def test_a_deleted_but_unstaged_file_is_left_out(self):
        # It is still in the index, and reading it fails. Skipping it is right:
        # the file is not in the tree, so it cannot hold anything.
        (self.root / "b.md").unlink()
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-count", body="echo \"count=$#\"; exit 0")
        done = run(self.root)
        self.assertIn("count=2", done.stdout, done.stderr)

    def test_staged_mode_checks_only_what_is_staged(self):
        (self.root / "hooks.d").mkdir()
        write_hook(
            self.root / "hooks.d" / "10-list",
            body='echo "files=$*"; exit 0',
        )
        (self.root / "new.rs").write_text("fn new() {}\n", encoding="utf-8")
        subprocess.run(  # noqa: S603
            ["git", "add", "new.rs"], cwd=self.root, check=True  # noqa: S607
        )
        done = run(self.root, "--staged")
        self.assertIn("files=new.rs", done.stdout)

    def test_an_empty_file_list_is_said_out_loud(self):
        # A green run over nothing must not read as a checked tree.
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-one")
        done = run(self.root, "--staged")
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("no staged file(s) to check", done.stderr)
        self.assertIn("0 staged file(s)", done.stdout)


class TellsTheHookWhereItIs(RepoCase):
    """The two environment variables a hook may rely on."""

    def test_root_and_mode_are_exported(self):
        (self.root / "hooks.d").mkdir()
        write_hook(
            self.root / "hooks.d" / "10-env",
            body='echo "root=$CLIO_PRECOMMIT_ROOT mode=$CLIO_PRECOMMIT_MODE"',
        )
        done = run(self.root)
        self.assertIn(f"root={self.root.resolve()} mode=all", done.stdout)

    def test_mode_is_staged_under_staged(self):
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-env", body='echo "mode=$CLIO_PRECOMMIT_MODE"')
        done = run(self.root, "--staged")
        self.assertIn("mode=staged", done.stdout)


class ReadsExtraHooksFromTheEnvironment(RepoCase):
    """CLIO_PRECOMMIT_HOOKS adds a file or a directory."""

    def setUp(self) -> None:
        super().setUp()
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-one")

    def test_a_named_directory_contributes_its_hooks(self):
        outside = self.root / "outside"
        outside.mkdir()
        write_hook(outside / "60-extra")
        done = run(self.root, "--list", env={"CLIO_PRECOMMIT_HOOKS": str(outside)})
        self.assertIn("60-extra", done.stdout, done.stderr)

    def test_a_named_file_is_taken_as_is(self):
        outside = self.root / "one-hook"
        write_hook(outside)
        done = run(self.root, "--list", env={"CLIO_PRECOMMIT_HOOKS": str(outside)})
        self.assertIn("one-hook", done.stdout, done.stderr)

    def test_the_same_hook_from_two_sources_runs_once(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.root / "hooks.d" / "70-shared").write_text("exit 0\n", encoding="utf-8")
        write_hook(outside / "70-shared")
        done = run(self.root, "--list", env={"CLIO_PRECOMMIT_HOOKS": str(outside)})
        self.assertEqual(1, done.stdout.count("70-shared"), done.stdout)


class ScopesHooks(RepoCase):
    """Which hooks a commit run takes, and how a hook says."""

    def declare(self, name: str, scope: str | None = None) -> Path:
        """Write a hook, optionally carrying a scope declaration."""
        (self.root / "hooks.d").mkdir(exist_ok=True)
        path = self.root / "hooks.d" / name
        header = f"# precommit-scope: {scope}\n" if scope else ""
        path.write_text(f"#!/bin/sh\n{header}exit 0\n", encoding="utf-8")
        path.chmod(0o755)
        return path

    def test_a_hook_that_says_nothing_runs_on_commit(self):
        # The default is the scope that cannot be lost. A check nobody has
        # classified yet runs on commit, so the worst case is a slow commit
        # rather than a check that quietly stopped running.
        self.declare("10-plain")
        done = run(self.root, "--commit", "--list")
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("10-plain (selected)", done.stdout)

    def test_a_manual_hook_is_left_out_of_a_commit_run(self):
        self.declare("20-slow", scope="manual")
        done = run(self.root, "--commit", "--list")
        self.assertIn("20-slow (skipped)", done.stdout)
        self.assertIn("not commit-scoped", done.stdout, done.stderr)

    def test_a_manual_hook_still_runs_when_the_run_asks_for_it(self):
        # `manual` means not on every commit, not never. A check that could only
        # be reached by committing would not be a check.
        self.declare("20-slow", scope="manual")
        done = run(self.root)
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("20-slow: pass", done.stdout)

    def test_a_commit_scope_is_the_same_as_saying_commit(self):
        self.declare("10-plain", scope="commit")
        done = run(self.root, "--commit", "--list")
        self.assertIn("10-plain (selected)", done.stdout)

    def test_an_unknown_scope_is_an_error_rather_than_a_guess(self):
        # An author who wrote `manul` meant one scope and would get the other,
        # and the other one is the one that stops running.
        self.declare("10-typo", scope="manul")
        done = run(self.root, "--commit")
        self.assertEqual(3, done.returncode, done.stdout + done.stderr)
        self.assertIn("not a scope", done.stderr)

    def test_a_commit_run_checks_only_the_staged_files(self):
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-list", body='echo "files=$*"; exit 0')
        (self.root / "new.rs").write_text("fn new() {}\n", encoding="utf-8")
        subprocess.run(  # noqa: S603
            ["git", "add", "new.rs"], cwd=self.root, check=True  # noqa: S607
        )
        done = run(self.root, "--commit")
        self.assertIn("files=new.rs", done.stdout, done.stdout + done.stderr)

    def test_a_failing_commit_scoped_hook_fails_the_run(self):
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-leak", body=LEAK_HOOK)
        (self.root / "new.rs").write_text(f"// {LEAK_MARKER}\n", encoding="utf-8")
        subprocess.run(  # noqa: S603
            ["git", "add", "new.rs"], cwd=self.root, check=True  # noqa: S607
        )
        done = run(self.root, "--commit")
        self.assertEqual(1, done.returncode, done.stdout + done.stderr)
        self.assertIn("10-leak: FAIL", done.stderr)

    def test_no_commit_scoped_hook_is_a_note_rather_than_a_blocked_commit(self):
        # There is no gate on this machine, so there is nothing to enforce, and
        # refusing every commit would only teach people to pass --no-verify.
        self.declare("20-slow", scope="manual")
        done = run(self.root, "--commit")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("no commit-scoped hook is registered", done.stderr)

    def test_a_broken_registration_still_fails_a_commit_run(self):
        # The leniency above must not swallow the failure this runner exists to
        # catch: a check registered here that can no longer run.
        (self.root / ".clio-hooks").mkdir()
        (self.root / ".clio-hooks" / "20-gone.py").symlink_to(self.root / "moved.py")
        self.declare("20-slow", scope="manual")
        done = run(self.root, "--commit")
        self.assertEqual(1, done.returncode, done.stdout + done.stderr)
        self.assertIn("broken hook", done.stderr)

    def test_a_windows_line_ending_still_declares_the_scope(self):
        # A checkout with CRLF endings would otherwise match no declaration and
        # keep the default, which for this hook means running a nine-second
        # suite on every commit on exactly the machines least able to afford it.
        (self.root / "hooks.d").mkdir()
        path = self.root / "hooks.d" / "20-crlf"
        path.write_bytes(b"#!/bin/sh\r\n# precommit-scope: manual\r\nexit 1\r\n")
        path.chmod(0o755)
        done = run(self.root, "--commit", "--list")
        self.assertIn("20-crlf (skipped)", done.stdout, done.stdout + done.stderr)

    def test_a_declaration_may_be_indented_or_padded(self):
        for name, line in {
            "10-indented": "   # precommit-scope: manual",
            "20-padded": "#precommit-scope:manual",
            "30-spaced": "#   precommit-scope :  manual  ",
        }.items():
            (self.root / "hooks.d").mkdir(exist_ok=True)
            path = self.root / "hooks.d" / name
            path.write_text(f"#!/bin/sh\n{line}\nexit 1\n", encoding="utf-8")
            path.chmod(0o755)
        done = run(self.root, "--commit", "--list")
        self.assertIn("0 of 3 hook(s) would run", done.stdout, done.stdout + done.stderr)

    def test_asking_for_a_manual_hook_by_name_in_a_commit_run_is_an_error(self):
        # The alternative would be a commit run that quietly ran nothing while
        # the caller believed a check had run.
        self.declare("20-slow", scope="manual")
        done = run(self.root, "--commit", "--only", "20-slow")
        self.assertEqual(3, done.returncode, done.stdout + done.stderr)
        self.assertIn("no hook named", done.stderr)

    def test_skipping_the_only_commit_hook_says_it_was_skipped(self):
        # The message has to match the facts: a check is registered, and this
        # run was told to leave it out. "none is registered" would be a lie a
        # reader could act on.
        self.declare("10-plain")
        done = run(self.root, "--commit", "--skip", "10-plain")
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("every commit-scoped hook was left out", done.stderr)
        self.assertNotIn("no commit-scoped hook is registered", done.stderr)

    def test_a_hook_that_cannot_be_read_is_an_error_not_a_skip(self):
        # A `.py` hook is found by its suffix, so an unreadable one is
        # discovered and then cannot be read. That has to be an error: the
        # alternative is a run that quietly checks one fewer thing.
        (self.root / "hooks.d").mkdir()
        path = self.root / "hooks.d" / "10-locked.py"
        path.write_text("#!/usr/bin/env python3\nprint('ran')\n", encoding="utf-8")
        path.chmod(0o000)
        self.addCleanup(path.chmod, 0o644)
        if os.access(path, os.R_OK):
            self.skipTest("this user can read a mode-000 file, so the case cannot be built")
        done = run(self.root)
        self.assertEqual(3, done.returncode, done.stdout + done.stderr)
        self.assertIn("cannot be read", done.stderr)

    def test_an_unreadable_extensionless_hook_is_not_discovered_at_all(self):
        # The other half, and the reason the case above uses a suffix: with no
        # recognised suffix the execute bit is what makes a file a hook, and a
        # file with no execute bit is not one. Nothing to report, because
        # nothing was ever registered as broken.
        (self.root / "hooks.d").mkdir()
        path = self.root / "hooks.d" / "10-locked"
        path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        path.chmod(0o000)
        self.addCleanup(path.chmod, 0o755)
        done = run(self.root)
        self.assertIn("hooks.d holds no hook", done.stderr, done.stdout + done.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
