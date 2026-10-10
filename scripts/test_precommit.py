#!/usr/bin/env python3
"""Tests for scripts/precommit.py: hook discovery, refusal to report a verdict it cannot support,
and hook selection.

Run: python3 scripts/test_precommit.py

Why these exist
---------------
This runner decides whether a set of checks ran. Every failure mode it has in
common is the same one: it reports success while a check silently did not run.
That is worse than no runner, because the reader has no way to tell. A missing
executable bit, a hook link whose target moved, a path the environment names
that is not there, an empty file list -- each of these was either a real defect
found while writing it or a way the run could go quietly green.

So the tests below are mostly negative. They pin the ways a run can lose a hook
without losing its verdict.

Each test builds a throwaway git repository in a temporary directory and runs
the real runner against it, so the discovery, the file list, and the exit codes
are exercised through the same path a person uses. Nothing here reads the real
checkout, so the suite gives the same answer on any machine -- but a repository
that reads nothing from the checkout still reads the machine's git
configuration, so every fixture is given a configuration of its own when it is
made, and a test holds it to that. The repository is built in a directory that
permits running an executable file, because a hook is one: on a host whose
temporary directory is mounted `noexec`, git refuses to run the fixtures' hooks
and blames their permissions, which are correct. That rule
lives in `scripts/exec_tempdir.py` and is shared with the other suite that needs
it, because two copies of it is two chances to get it wrong.

The shared fixtures are in `precommit_testkit.py`. The scope tests are in
`test_precommit_scopes.py`, the git hook shims in `test_precommit_shim.py`, and
`--commit --all-files` in `test_precommit_all_files.py`.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from exec_tempdir import CANDIDATE_PARENTS, can_execute, exec_tempdir, make_exec_dir  # noqa: E402
from precommit_testkit import RUNNER, RepoCase, build_repo, run, write_hook  # noqa: E402


class FixturesLandWhereGitCanRunThem(unittest.TestCase):
    """The fixture repository is built somewhere a hook file can be executed.

    Git will not run a hook from a filesystem mounted `noexec`, and it reports
    that as the hook "not being set as executable" — a statement about a
    permission that is in fact correct. On such a host every git-fixture test
    below failed for a reason that had nothing to do with the tree, and the
    repository's mandatory pre-commit gate could not pass at all.
    """

    def test_the_fixture_directory_permits_running_a_hook(self):
        with exec_tempdir() as tmp:
            self.assertTrue(
                can_execute(tmp),
                f"{tmp} was chosen for the fixtures but refuses to run a hook",
            )

    def test_a_hook_git_consults_really_runs_in_the_fixture_directory(self):
        # The failure this pins is not visible from the runner's own output: the
        # runner reports the hook passed, because git never ran it. So the check
        # is git's, on a real commit, with a hook that must have executed.
        with exec_tempdir() as tmp:
            root = Path(tmp)
            build_repo(root)
            hooks = root / ".git" / "hooks"
            write_hook(hooks / "pre-commit", "echo 'the fixture hook ran'")
            (root / "a.rs").write_text("fn main() { }\n", encoding="utf-8")
            self.git_add_and_commit(root)
            done = subprocess.run(  # noqa: S603
                ["git", "commit", "-qm", "second", "--allow-empty"],
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(0, done.returncode, done.stdout + done.stderr)
            self.assertNotIn("was ignored because", done.stderr, done.stderr)

    @staticmethod
    def git_add_and_commit(root: Path) -> None:
        subprocess.run(["git", "add", "-A"], cwd=root, check=True)  # noqa: S603, S607
        subprocess.run(  # noqa: S603, S607
            ["git", "commit", "-qm", "second"],
            cwd=root,
            check=True,
        )

    def test_a_candidate_that_refuses_execution_is_abandoned_not_used(self):
        # The fallback is the whole fix, and this host's temporary directory is
        # executable, so the branch that matters is never taken here. Rejecting
        # the first candidate by hand exercises it: the helper must move on to the
        # next one rather than hand back a directory whose hooks git will not run.
        asked: list[str] = []

        def rejects_only_the_first(directory: str) -> bool:
            asked.append(directory)
            return False if len(asked) == 1 else can_execute(directory)

        with exec_tempdir(probe=rejects_only_the_first) as tmp:
            self.assertGreaterEqual(
                len(asked), 2, "the first candidate was refused, so another must be tried"
            )
            self.assertTrue(
                can_execute(tmp), f"{tmp} is the directory that was finally used"
            )

    def test_a_host_with_nowhere_to_run_a_hook_skips_with_a_reason(self):
        # Better one stated skip than twenty failures that each blame a file
        # permission which is in fact correct.
        with self.assertRaises(unittest.SkipTest) as raised:
            exec_tempdir(probe=lambda _directory: False, purpose="a git hook")
        self.assertIn("permits running a git hook", str(raised.exception))

    def test_a_home_directory_is_never_chosen_for_a_fixture(self):
        # The fixture tree is a repository with a hook in it and a little history.
        # Dropping that into a home directory puts megabytes of throwaway work
        # where a person keeps their files, and one left behind outlives the run
        # that made it. A home directory is not scratch space, so it is not a
        # candidate at all.
        expanded = {os.path.expanduser(parent) for parent in CANDIDATE_PARENTS if parent}
        self.assertNotIn(
            os.path.expanduser("~"),
            expanded,
            f"a home directory must not be a fixture candidate; candidates are {sorted(expanded)}",
        )

    def test_a_directory_kept_for_another_process_outlives_the_maker(self):
        # `make_exec_dir` hands its path to a caller in another process, so it must
        # not remove the directory when the process that made it exits. That is the
        # whole difference from `exec_tempdir`, and getting it wrong deletes the
        # fixture out from under the caller.
        directory = make_exec_dir()
        self.addCleanup(shutil.rmtree, directory, True)
        self.assertTrue(os.path.isdir(directory), f"{directory} was made and then removed")
        self.assertTrue(can_execute(directory), f"{directory} cannot run an executable file")

    def test_a_directory_the_caller_keeps_is_absolute(self):
        # The caller changes directory while it works, so a relative path would be
        # resolved against whatever happened to be current at the time it was used.
        directory = make_exec_dir()
        self.addCleanup(shutil.rmtree, directory, True)
        self.assertTrue(os.path.isabs(directory), f"{directory} is not absolute")

    def test_a_host_with_nowhere_to_run_a_hook_reports_no_directory_rather_than_raising(self):
        # The non-test caller has no way to handle a skip, so this entry point
        # answers an empty path and lets the caller say what to do about it.
        self.assertEqual("", make_exec_dir(probe=lambda _directory: False))

    def test_a_missing_build_tree_candidate_is_created_on_first_use(self):
        # A fresh checkout has no target/ yet. The helper must create its
        # build-tree candidate instead of skipping it, or a noexec host
        # finds nowhere to run a hook and every exec test skips.
        import exec_tempdir

        with tempfile.TemporaryDirectory() as root:
            fake_scripts = Path(root) / "scripts"
            fake_scripts.mkdir()
            fake_target = str(Path(root) / "target")
            old = exec_tempdir.SCRIPTS
            exec_tempdir.SCRIPTS = fake_scripts
            try:
                directory = make_exec_dir(probe=lambda _d: True, parents=[fake_target])
            finally:
                exec_tempdir.SCRIPTS = old
            self.assertTrue(directory, "a missing build-tree candidate must be created")
            self.addCleanup(shutil.rmtree, directory, True)
            self.addCleanup(shutil.rmtree, fake_target, True)
            self.assertTrue(os.path.isdir(directory), f"{directory} was not created")

    def test_the_shared_rule_lives_in_one_place(self):
        # Both suites in this repository need this rule. A second copy is a second
        # thing to update when the first one moves, so the helper that owns it is
        # named here rather than reimplemented.
        self.assertTrue(
            (Path(RUNNER).parent / "exec_tempdir.py").is_file(),
            "scripts/exec_tempdir.py is the one place this rule is written",
        )


class AsksGitNothingOfTheHost(RepoCase):
    """A fixture repository answers for itself, whatever the machine has set.

    The suite builds a throwaway repository so the answer is the same everywhere.
    A repository still reads the machine's global and system configuration, so
    the promise only holds while the fixture carries its own answers, and the
    only thing that keeps it carrying them is a test that says so.
    """

    #: A global git configuration a real machine can have, with every setting in
    #: it measured to break a commit in a repository that has not answered for
    #: itself: an identity of its own, commits to be signed with a key it does
    #: not have, hooks looked for in a directory that is not there, and line
    #: endings rewritten on the way back out of the index.
    HOST_GITCONFIG = (
        "[user]\n\tname = someone\n\temail = someone@host\n"
        "[commit]\n\tgpgsign = true\n"
        "[core]\n\tautocrlf = true\n\thooksPath = /nonexistent-shared-hooks\n"
    )

    def on_a_host_with_a_git_configuration(self) -> dict[str, str]:
        """An environment whose git configuration is the machine's, not ours."""
        home = self.root.parent / f"{self.root.name}-home"
        home.mkdir()
        self.addCleanup(shutil.rmtree, home, True)
        (home / ".gitconfig").write_text(self.HOST_GITCONFIG, encoding="utf-8")
        environment = dict(os.environ)
        environment["HOME"] = str(home)
        environment["GIT_CONFIG_NOSYSTEM"] = "1"
        return environment

    def git_commit(self, message: str, environment: dict[str, str]) -> subprocess.CompletedProcess:
        return subprocess.run(  # noqa: S603
            ["git", "commit", "-qm", message, "--allow-empty"],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )

    def test_a_commit_needs_nothing_from_the_machine_that_runs_it(self):
        # The failure this pins is invisible from the runner's own output and has
        # nothing to do with the tree: git refuses the commit before the runner
        # is asked anything, so a suite that cannot commit reports a green
        # runner failing for a reason it does not own. That is how a suite built
        # to be the same everywhere passed on a laptop and failed on a build
        # machine with no identity configured.
        done = self.git_commit("unasked", self.on_a_host_with_a_git_configuration())
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)

    def test_a_hook_git_consults_is_the_one_the_fixture_installed(self):
        # The other half. The identity can be carried in the repository and the
        # hook directory still be somewhere else, in which case the commit
        # succeeds and the check never runs -- which is the failure this runner
        # was written to refuse, arriving through the runner's own test suite.
        write_hook(self.root / ".git" / "hooks" / "pre-commit", "echo 'the fixture hook ran'")
        done = self.git_commit("hooked", self.on_a_host_with_a_git_configuration())
        self.assertEqual(0, done.returncode, done.stdout + done.stderr)
        self.assertIn("the fixture hook ran", done.stdout + done.stderr)


class DiscoversHooks(RepoCase):
    """Every registered hook is found, whatever its file mode."""

    def test_a_tracked_hook_directory_is_searched(self):
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-one")
        done = run(self.root, "--list")
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("10-one", done.stdout)

    def test_a_machine_local_hook_directory_is_searched(self):
        (self.root / ".clio-hooks").mkdir()
        write_hook(self.root / ".clio-hooks" / "20-local")
        done = run(self.root, "--list")
        self.assertIn("20-local", done.stdout, done.stderr)

    def test_a_python_hook_without_the_executable_bit_still_runs(self):
        # The defect this pins: the runner once required the execute bit, so a
        # hook whose mode was lost to a checkout or a `cp` was skipped and the
        # run still reported success. A .py hook is started through the
        # interpreter, so the bit is not what makes it a hook.
        (self.root / "hooks.d").mkdir()
        target = self.root / "hooks.d" / "30-no-bit.py"
        target.write_text("#!/usr/bin/env python3\nprint('ran')\n", encoding="utf-8")
        target.chmod(0o644)
        done = run(self.root)
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("30-no-bit: pass", done.stdout)

    def test_an_extensionless_hook_needs_the_executable_bit(self):
        # The other half of the contract: with no recognised suffix, nothing
        # says "this is a script", so the file has to prove it.
        (self.root / "hooks.d").mkdir()
        target = self.root / "hooks.d" / "40-no-suffix"
        target.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        target.chmod(0o644)
        done = run(self.root)
        self.assertEqual(1, done.returncode, "an unreadable hook file must not pass as checked")
        self.assertIn("no hook is registered", done.stderr)
        target.chmod(0o755)
        self.assertEqual(0, run(self.root).returncode, "with the bit it is a hook")

    def test_an_executable_extensionless_hook_is_a_hook(self):
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "40-plain")
        done = run(self.root)
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("40-plain: pass", done.stdout)

    def test_a_non_hook_file_is_not_run(self):
        (self.root / "hooks.d").mkdir()
        (self.root / "hooks.d" / "README.md").write_text("notes\n", encoding="utf-8")
        done = run(self.root)
        self.assertEqual(1, done.returncode, "a run with no hook must not claim to pass")
        self.assertIn("no hook is registered", done.stderr)

    def test_the_number_prefix_orders_hooks_across_sources(self):
        # Sorting per source directory would put a machine-local hook last
        # whatever it is called, so a `20-` from outside would run after a `30-`
        # from inside. The prefix is the only ordering signal a reader has.
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-first")
        write_hook(self.root / "hooks.d" / "30-third")
        (self.root / ".clio-hooks").mkdir()
        write_hook(self.root / ".clio-hooks" / "20-second")
        done = run(self.root, "--list")
        listed = [line for line in done.stdout.splitlines() if "selected" in line]
        self.assertEqual(
            ["10-first", "20-second", "30-third"],
            [line.split()[0] for line in listed],
            done.stdout,
        )


class RefusesToReportAVerdictItCannotSupport(RepoCase):
    """A gate that stopped running must not look like a gate that passed."""

    def test_a_hook_link_whose_target_is_gone_fails_the_run(self):
        local = self.root / ".clio-hooks"
        local.mkdir()
        (local / "40-dangling").symlink_to("../elsewhere/40-dangling")
        done = run(self.root, "--list")
        self.assertEqual(1, done.returncode, done.stderr)
        self.assertIn("broken hook", done.stderr)
        self.assertIn("target is gone", done.stderr)

    def test_a_hook_directory_holding_nothing_is_only_a_note(self):
        # No machine-local hooks is a legitimate state, so it must not fail.
        (self.root / ".clio-hooks").mkdir()
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-one")
        done = run(self.root)
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("holds no hook", done.stderr)

    def test_an_environment_path_that_is_not_there_is_an_error(self):
        done = run(self.root, "--list", env={"CLIO_PRECOMMIT_HOOKS": str(self.root / "gone")})
        self.assertEqual(3, done.returncode, done.stderr)
        self.assertIn("not there", done.stderr)

    def test_a_failing_hook_fails_the_run_and_is_named(self):
        # Failures go to stderr on purpose, so `precommit.py >/dev/null` still
        # shows what broke. That is why the assertion reads the combined output.
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "50-bad", body="echo 'it is broken'; exit 4")
        done = run(self.root)
        self.assertEqual(1, done.returncode, done.stdout + done.stderr)
        self.assertIn("50-bad: FAIL", done.stdout + done.stderr)
        self.assertIn("it is broken", done.stdout + done.stderr)
        self.assertIn("exit 4", done.stderr)

    def test_every_hook_runs_even_when_an_earlier_one_fails(self):
        # A first failure must not hide the state of the rest.
        (self.root / "hooks.d").mkdir()
        write_hook(self.root / "hooks.d" / "10-bad", body="exit 1")
        write_hook(self.root / "hooks.d" / "20-good")
        done = run(self.root)
        self.assertEqual(1, done.returncode)
        self.assertIn("20-good: pass", done.stdout)

    def test_running_outside_a_repository_is_an_error_not_a_pass(self):
        with tempfile.TemporaryDirectory() as outside:
            done = subprocess.run(  # noqa: S603
                [sys.executable, str(RUNNER), "--list"],
                cwd=outside,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(3, done.returncode, done.stdout + done.stderr)


class SelectsHooks(RepoCase):
    """--only and --skip narrow the run, and a typo is an error."""

    def setUp(self) -> None:
        super().setUp()
        (self.root / "hooks.d").mkdir()
        for name in ("10-one", "20-two", "30-three"):
            write_hook(self.root / "hooks.d" / name)

    def test_only_runs_the_named_hook(self):
        done = run(self.root, "--only", "20-two")
        self.assertEqual(0, done.returncode, done.stderr)
        self.assertIn("20-two: pass", done.stdout)
        self.assertNotIn("10-one:", done.stdout)

    def test_skip_leaves_the_rest(self):
        done = run(self.root, "--skip", "20-two")
        self.assertIn("10-one: pass", done.stdout)
        self.assertIn("30-three: pass", done.stdout)
        self.assertNotIn("20-two:", done.stdout)

    def test_an_unknown_name_is_an_error_rather_than_an_empty_run(self):
        # Silently running nothing would be a green run that checked nothing.
        done = run(self.root, "--only", "no-such-hook")
        self.assertEqual(3, done.returncode, done.stderr)
        self.assertIn("no hook named", done.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
