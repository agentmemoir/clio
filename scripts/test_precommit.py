#!/usr/bin/env python3
"""Tests for scripts/precommit.py.

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

RUNNER = Path(__file__).resolve().parent / "precommit.py"
SHIM = Path(__file__).resolve().parent / "git-hooks" / "pre-commit"
MERGE_SHIM = Path(__file__).resolve().parent / "git-hooks" / "pre-merge-commit"

#: A hook that fails when any file it is given contains this marker. Stands in
#: for a check that reads the change, which is what a commit hook is for.
#:
#: The marker is assembled at run time from two halves, because the hook is
#: itself a staged file: spelled out in one piece, the hook would reject its own
#: source and every test that installs it would fail for the wrong reason. That
#: is not a hypothetical, it is what the first version of this string did.
LEAK_HOOK = (
    'm="LE""AK"\n'
    'grep -l "$m" "$@" >/dev/null 2>&1 && { echo "found a leak"; exit 1; }\n'
    "exit 0"
)
#: What a file has to contain for the hook above to reject it.
LEAK_MARKER = "LEAK"

#: The git configuration a fixture repository answers for itself.
#:
#: Git reads three configurations, and its own is read last, so a value written
#: here beats anything the machine running the suite has set. That is the whole
#: point of this table: a throwaway repository built to be independent of the
#: machine is not independent while a commit in it can still ask the machine who
#: it is. One place, applied once when the repository is made, so every `git`
#: call afterwards -- the suite's own, the runner's, and the ones git makes while
#: running a hook -- is covered without each having to remember a flag.
#:
#: Every key is here because a host that set it was measured failing this suite.
#: With no identity, every commit is refused. `commit.gpgsign` refuses them too,
#: on any machine without a signing key, and a `-c` flag naming the author does
#: not help: `-c` overrides the keys it names and nothing else. A `core.autocrlf`
#: rewrites a freshly checked-out hook's shebang to `#!/bin/sh\r`, which the
#: kernel cannot execute. A `core.fsmonitor` that is not a working daemon fails
#: the commit outright. `core.hooksPath` is not here because it needs a path from
#: this repository rather than a constant; it is written by `configure`.
FIXTURE_CONFIG: dict[str, str] = {
    "user.name": "t",
    "user.email": "t@e",
    "commit.gpgsign": "false",
    "core.autocrlf": "false",
    "core.fsmonitor": "false",
}


def configure(root: Path) -> None:
    """Give the repository at `root` a git configuration of its own.

    `core.hooksPath` gets an absolute path on purpose. A host may point it at a
    directory of its own, and then git looks there instead of here, finds no
    `pre-commit`, and commits anyway without saying so -- the one outcome this
    suite exists to refuse. A relative path would not do: git resolves one
    against the top of the working tree, and in a linked worktree that tree's
    `.git` is a file, not a directory. The absolute path of the main checkout's
    hook directory is what git reaches on its own anyway, because `.git/hooks` is
    shared by every worktree of a repository.
    """
    for key, value in FIXTURE_CONFIG.items():
        subprocess.run(  # noqa: S603
            ["git", "config", "--local", key, value], cwd=root, check=True  # noqa: S607
        )
    subprocess.run(  # noqa: S603
        ["git", "config", "--local", "core.hooksPath", str(root / ".git" / "hooks")],
        cwd=root,
        check=True,
    )


def build_repo(root: Path) -> None:
    """A minimal repository with one tracked text file and one tracked binary.

    Configured before the first commit, so the identity that commit needs comes
    from the repository rather than from a flag on the call that happens to make
    it.
    """
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)  # noqa: S603, S607
    configure(root)
    (root / "a.rs").write_text("fn main() {}\n", encoding="utf-8")
    (root / "b.md").write_text("# doc\n", encoding="utf-8")
    (root / "c.bin").write_bytes(b"\x00\x01\x02\xff\xfe")
    (root / "sub").mkdir()
    (root / "sub" / "d.rs").write_text("// nested\n", encoding="utf-8")
    subprocess.run(  # noqa: S603
        ["git", "add", "-A"], cwd=root, check=True  # noqa: S607
    )
    subprocess.run(["git", "commit", "-qm", "init"], cwd=root, check=True)  # noqa: S603, S607


def run(root: Path, *args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
    """Run the real runner inside `root`."""
    environment = dict(os.environ)
    environment.pop("CLIO_PRECOMMIT_HOOKS", None)
    if env:
        environment.update(env)
    return subprocess.run(  # noqa: S603
        [sys.executable, str(RUNNER), *args],
        cwd=root,
        capture_output=True,
        text=True,
        env=environment,
        check=False,
    )


def write_hook(path: Path, body: str = "exit 0", executable: bool = True) -> Path:
    """Write an executable-free-by-default shell hook."""
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    if executable:
        path.chmod(0o755)
    return path


class RepoCase(unittest.TestCase):
    """A fresh repository per test, removed afterwards."""

    def setUp(self) -> None:
        self._tmp = exec_tempdir()
        self.root = Path(self._tmp.name)
        # Registered before the repository is built, because a build that fails
        # ends the test there. Cleaning up afterwards would leave the directory
        # behind, and the run that failed for one reason would then also litter
        # the machine it failed on.
        self.addCleanup(self._tmp.cleanup)
        build_repo(self.root)


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


class TheCommitHookScript(RepoCase):
    """The shell shim a machine installs as .git/hooks/pre-commit."""

    def install(self, merge: bool = True) -> None:
        """Copy the runner and the shims in, and link them up as git wants them.

        `merge` controls the pre-merge-commit link, because the point of the
        tests that pass False is to show what git does without it.
        """
        (self.root / "scripts" / "git-hooks").mkdir(parents=True)
        (self.root / "scripts" / "precommit.py").write_text(
            RUNNER.read_text(encoding="utf-8"), encoding="utf-8"
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
        return subprocess.run(  # noqa: S603
            ["git", *args],  # noqa: S607
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
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
