from __future__ import annotations

"""Kept-run (<kind>.prev) and replay stand-ins for the published log (<kind>.log)."""

import contextlib
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from exec_tempdir import exec_tempdir
from gate import *  # noqa: F403 - re-exports every engine name the tests reference
from gate_cache import PUBLISH_LOCK_NAME, _body_only, _publish_lock, _publish_log
from gate_kinds import EXECUTED_LOG_MARKER, REPLAYED_LOG_MARKER

def state(head: str, contents=(), changed=()) -> TreeState:
    return TreeState(head, tuple(contents), tuple(changed))

def published(name: str, log_dir: Path) -> str:
    """What the wrapper published under a kind, or "" when it published nothing.

    A missing log is the failure these checks are looking for, so it has to
    come back as a value the comparison rejects rather than as an exception
    that hides which check failed.
    """
    try:
        return (log_dir / name).read_text()
    except OSError:
        return ""

PUBLISHED_NAME = re.compile(r"<kind>\.log|CLIO_GATE_LOG_DIR")
KEPT_NAME = re.compile(r"<kind>\.prev")
TEXT_SUFFIXES = (".md", ".py", ".sh", ".yml", ".yaml", ".toml", ".txt", ".json")
TEST_CODE_SUFFIXES = {".py", ".rs", ".sh", ".yml", ".yaml", ".toml", ".json"}


def _is_test_surface_file(name: str) -> bool:
    path = Path(name)
    return (
        any(part.lower() in {"test", "tests"} for part in path.parts[:-1])
        or (
            path.suffix.lower() in TEST_CODE_SUFFIXES
            and re.search(r"(?:^|[-_])(?:tests?|testkit|self[_-]?tests?)(?:[-_.]|$)", path.stem) is not None
        )
    )


class DirectTests(unittest.TestCase):
    def test_direct_cache_reuse_and_mid_run_change(self) -> None:
        checks: list[tuple[str, bool]] = []
        direct = state("h1", changed=("crates/a/src/lib.rs",))
        with tempfile.TemporaryDirectory() as tmp:
            counter = Path(tmp) / "count"
            cache = Path(tmp) / "cache"
            cmd = ["/bin/sh", "-c", f'echo run >> "{counter}"']
            red = ["/bin/sh", "-c", f'echo run >> "{counter}"; exit 7']
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache)})
            root_patch = mock.patch("gate.repo_root", return_value="/repo")
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, mock.patch("gate.read_tree_state", return_value=direct), quiet[0], quiet[1]:
                checks.append(("first run executes and passes", run_gate(cmd) == 0))
                checks.append(("same tree reuses the green verdict", run_gate(cmd) == 0 and counter.read_text().count("run") == 1))
                checks.append(("red verdict returned", run_gate(red) == 7))
                checks.append(("red verdict replayed, not rerolled", run_gate(red) == 7 and counter.read_text().count("run") == 2))
                checks.append(("--force re-executes", run_gate(cmd, force=True) == 0 and counter.read_text().count("run") == 3))
            with env_patch, root_patch, mock.patch(
                "gate.read_tree_state",
                side_effect=[state("h1", contents=(("crates/a/src/lib.rs", "one"),), changed=("crates/a/src/lib.rs",)), state("h1", contents=(("crates/a/src/lib.rs", "two"),), changed=("crates/a/src/lib.rs",))],
            ), quiet[0], quiet[1]:
                run_gate(cmd)
            with env_patch, root_patch, mock.patch("gate.read_tree_state", return_value=state("h1", contents=(("crates/a/src/lib.rs", "one"),), changed=("crates/a/src/lib.rs",))), quiet[0], quiet[1]:
                run_gate(cmd)
                checks.append(("a mid-run change is not cached", counter.read_text().count("run") == 5))

        for _name, _ok in checks:
            self.assertTrue(_ok, _name)

    def test_path_separation(self) -> None:
        checks: list[tuple[str, bool]] = []
        with tempfile.TemporaryDirectory() as tmp:
            counter = Path(tmp) / "count"
            cache = Path(tmp) / "cache"
            red = ["/bin/sh", "-c", f'echo run >> "{counter}"; exit 7']
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache)})
            root_patch = mock.patch("gate.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "gate.read_tree_state",
                return_value=state("h1", contents=(("crates/a/src/lib.rs", "path"),), changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, mock.patch.dict(os.environ, {"PATH": "/one"}), quiet[0], quiet[1]:
                run_gate(red)
                checks.append(("red stored under a PATH", counter.read_text().count("run") == 1))
            with env_patch, root_patch, tree_patch, mock.patch.dict(os.environ, {"PATH": "/two"}), quiet[0], quiet[1]:
                checks.append(("red re-runs under a different PATH", run_gate(red) == 7 and counter.read_text().count("run") == 2))
            with env_patch, root_patch, tree_patch, mock.patch.dict(os.environ, {"PATH": "/two"}), quiet[0], quiet[1]:
                checks.append(("red replays under the same PATH", run_gate(red) == 7 and counter.read_text().count("run") == 2))
        for _name, _ok in checks:
            self.assertTrue(_ok, _name)


class StandInTests(unittest.TestCase):
    # The two commands the wrapper used to treat as one, driven end to end.
    #
    # A stand-in executable named `cargo` is used instead of the real one, because
    # the classification reads the file name and nothing else about the command, so
    # the wrapper's decision can be exercised without a build. Each run appends to a
    # counter and prints the arguments it was handed, so the stored logs can be told
    # apart as well as the runs.
    def test_cargo_stand_incargo_stand_in(self) -> None:
        checks: list[tuple[str, bool]] = []
        with exec_tempdir(purpose="a stand-in cargo command") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "cargo"
            stand_in.write_text(f'#!/bin/sh\necho run >> "{counter}"\necho "invoked: $*"\nexit 7\n')
            stand_in.chmod(0o755)
            log_dir = Path(tmp) / "logs"
            run = [str(stand_in), "nextest", "run", "-p", "a"]
            lookup = [str(stand_in), "nextest", "list", "--workspace", "--locked"]
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("gate.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "gate.read_tree_state",
                return_value=state("h1", contents=(("crates/a/src/lib.rs", "cmd"),), changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                checks.append(("a test run stores its red verdict", run_gate(run, log_dir=str(log_dir)) == 7))
                checks.append(
                    ("the same run replays it", run_gate(run, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 1)
                )
                checks.append(
                    (
                        "a test lookup runs instead of replaying the run",
                        run_gate(lookup, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 2,
                    )
                )
                checks.append(
                    ("a lookup runs again without a cache slot", run_gate(lookup, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 3)
                )
                checks.append(
                    ("the run still replays afterwards", run_gate(run, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 3)
                )
                metas = list((cache_root() / repo_key("/repo")).glob("*/meta.json"))
                checks.append(("only the real test run has a stored verdict", len(metas) == 1))
            lookup_log = published(f"{NEXTEST_LOOKUP_KIND}.log", log_dir)
            run_log = published("test.log", log_dir)
            checks.append(("the lookup publishes under its own kind", "nextest list" in lookup_log))
            checks.append(("the lookup log marks an executed command", lookup_log.startswith(EXECUTED_LOG_MARKER.decode())))
            checks.append(
                (
                    "the run's verdict is not the lookup's",
                    "nextest run" in run_log and "nextest list" not in run_log,
                )
            )
            checks.append(
                (
                    "each publishes its own exit code",
                    published(f"{NEXTEST_LOOKUP_KIND}.exit", log_dir).strip() == "7"
                    and published("test.exit", log_dir).strip() == "7",
                )
            )

        for _name, _ok in checks:
            self.assertTrue(_ok, _name)

    # Two `make test` runs that differ only in the package list the environment
    # supplies. The scoping code reads it, so the second run really is a different
    # command, and a key that ignored it handed the second run the first run's
    # verdict while publishing a log naming the first package list.
    def test_make_stand_inmake_stand_in(self) -> None:
        checks: list[tuple[str, bool]] = []
        with exec_tempdir(purpose="a stand-in make command") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "make"
            stand_in.write_text(f'#!/bin/sh\necho run >> "{counter}"\necho "PACKAGES=$PACKAGES"\nexit 3\n')
            stand_in.chmod(0o755)
            cmd = [str(stand_in), "test"]
            log_dir = Path(tmp) / "logs"
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("gate.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "gate.read_tree_state",
                return_value=state("h1", contents=(("crates/a/src/lib.rs", "pkg"),), changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                with mock.patch.dict(os.environ, {"PACKAGES": "-p a"}):
                    first = run_gate(cmd, log_dir=str(log_dir))
                with mock.patch.dict(os.environ, {"PACKAGES": "-p b"}):
                    second = run_gate(cmd, log_dir=str(log_dir))
                checks.append(
                    (
                        "a package list from the environment separates cache entries",
                        first == 3 and second == 3 and counter.read_text().count("run") == 2,
                    )
                )
            checks.append(
                (
                    "the published log names the package list that ran",
                    published("test.log", log_dir).endswith("PACKAGES=-p b\n"),
                )
            )

        for _name, _ok in checks:
            self.assertTrue(_ok, _name)

    # The log a run replaced is the evidence that the run before it happened, and
    # overwriting it means a reproduction cannot be read back once the fix lands.
    # The stand-in changes its own output and exit code on each run, and the tree
    # moves between them so the second run is a fresh one rather than a replay,
    # which is what an agent editing code does.
    #
    # This is a check of its own rather than three more assertions on the make
    # stand-in above, which already runs one command twice over one log
    # directory. Folding them together would couple keeping the replaced log to
    # telling two package lists apart, and a failure would then be reported under
    # whichever name the reader happened to be looking for.
    def test_publish_keeps_the_replaced_logpublish_keeps_the_replaced_log(self) -> None:
        checks: list[tuple[str, bool]] = []
        with exec_tempdir(purpose="a stand-in command that changes between runs") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "changing"
            stand_in.write_text(
                f'#!/bin/sh\n'
                f'n=$(cat "{counter}" 2>/dev/null || echo 0)\n'
                f'n=$((n+1))\n'
                f'echo "$n" > "{counter}"\n'
                f'if [ "$n" -eq 1 ]; then echo "before the fix"; exit 101; fi\n'
                f'echo "after the fix"; exit 0\n'
            )
            stand_in.chmod(0o755)
            cmd = [str(stand_in)]
            log_dir = Path(tmp) / "logs"
            head = {"id": "h1"}
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("gate.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "gate.read_tree_state",
                side_effect=lambda *_: state(head["id"], changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                first = run_gate(cmd, log_dir=str(log_dir))
                head["id"] = "h2"
                second = run_gate(cmd, log_dir=str(log_dir))
                # Read what was published here, before the next run, so each check
                # below reports the state its own name describes rather than the
                # last state the directory happened to reach.
                kept = published("command.prev", log_dir)
                newest = published("command.log", log_dir)
                newest_code = published("command.exit", log_dir)
                replay = run_gate(cmd, log_dir=str(log_dir))
                replay_log = published("command.log", log_dir)
                kept_after_replay = published("command.prev", log_dir)
                # A shell redirect truncates the published log before the command
                # starts, which is how the empty file gets there. It must not cost
                # the run already kept.
                (log_dir / "command.log").write_text("")
                head["id"] = "h3"
                after_empty = run_gate(cmd, log_dir=str(log_dir))
                kept_after_empty = published("command.prev", log_dir)
            checks.append(
                (
                    "the run a later run replaced is still readable",
                    first == 101 and second == 0
                    and kept.splitlines()[-1:] == ["before the fix"]
                    and newest.endswith("after the fix\n")
                    and newest.startswith(EXECUTED_LOG_MARKER.decode()),
                )
            )
            checks.append(
                (
                    "the kept run names the exit code it exited with",
                    kept.startswith("exit: 101\n") and newest_code.strip() == "0",
                )
            )
            checks.append(
                (
                    "replaying a verdict does not invent an earlier run",
                    replay == 0
                    and kept_after_replay.startswith("exit: 101\n")
                    and replay_log.endswith("after the fix\n")
                    and replay_log.startswith(REPLAYED_LOG_MARKER.decode()),
                )
            )
            checks.append(
                (
                    "an emptied published log does not cost the kept run",
                    after_empty == 0 and kept_after_empty == kept,
                )
            )

        for _name, _ok in checks:
            self.assertTrue(_ok, _name)

    def test_every_surface_names_the_kept_run_too(self) -> None:
        checks: list[tuple[str, bool]] = []
        # Tracked files only. A checkout carries surfaces this repository does not
        # own -- a link to another checkout's instructions, a machine-local hook --
        # and reading those would report a half-described contract in a file the
        # next person cannot edit from here. Git is also the honest answer to
        # "which files are ours", rather than a list of directories to exclude.
        root = Path(__file__).resolve().parent.parent
        try:
            listed = subprocess.run(
                ["git", "ls-files", "-z", "--", *(f"*{suffix}" for suffix in TEXT_SUFFIXES)],
                cwd=root,
                capture_output=True,
                text=True,
                check=True,
            ).stdout
        except (OSError, subprocess.CalledProcessError) as error:
            checks.append(("every surface naming the published log also names the kept run", False))
            print(f"self-test note: cannot list tracked files: {error}")
            return
        half_described: list[str] = []
        for name in listed.split("\0"):
            if not name:
                continue
            if _is_test_surface_file(name):
                continue
            path = root / name
            try:
                text = path.read_text(errors="replace")
            except OSError:
                continue
            if PUBLISHED_NAME.search(text) and not KEPT_NAME.search(text):
                half_described.append(name)
        checks.append(
            (
                "every surface naming the published log also names the kept run",
                not half_described,
            )
        )
        for offender in sorted(half_described):
            print(f"self-test note: describes publishing but not the kept run: {offender}")

        for _name, _ok in checks:
            self.assertTrue(_ok, _name)


class PublishPairTests(unittest.TestCase):
    """The newest published pair describes one run, and it names that run."""

    def test_a_failed_exit_write_does_not_leave_a_newer_log_beside_an_older_code(self) -> None:
        # The exit code is the verdict a reader trusts. When the run cannot write
        # it, nothing may be published, or the pair holds two runs: a newer log
        # beside an older exit code. That pair is issue #186, read as a passing
        # tree because the published exit file still described the run before the
        # failing one.
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            log_dir.mkdir()
            older = EXECUTED_LOG_MARKER + b"OLD RUN\n"
            (log_dir / "command.log").write_bytes(older)
            (log_dir / "command.exit").write_text("0\n")
            newer = Path(tmp) / "log"
            newer.write_bytes(b"NEW FAILING RUN\n")
            # A directory stands where the exit file goes, so replacing it fails.
            (log_dir / "command.exit").unlink()
            (log_dir / "command.exit").mkdir()
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            run_line = b"gate: run worktree=/w tree=abc finished=?\n"
            with quiet[0], quiet[1]:
                result = _publish_log(newer, str(log_dir), "command", 2, run_line=run_line)

            self.assertIsNone(result)
            self.assertEqual(
                older,
                (log_dir / "command.log").read_bytes(),
                "a newer log was published beside an older exit code",
            )

    def test_a_repeat_of_the_same_run_invents_no_earlier_run(self) -> None:
        # The record names its run, so two byte-identical commands differ by that
        # line alone. The store's older rule still holds: identical output is not a
        # new run, and a false earlier run is the worse error.
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            log_dir.mkdir()
            first = Path(tmp) / "one"
            first.write_bytes(b"same output\n")
            second = Path(tmp) / "two"
            second.write_bytes(b"same output\n")
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            first_line = b"gate: run worktree=/w tree=aaa finished=2026-01-01T00:00:00Z\n"
            second_line = b"gate: run worktree=/w tree=aaa finished=2026-01-02T00:00:00Z\n"
            with quiet[0], quiet[1]:
                _publish_log(first, str(log_dir), "command", 0, run_line=first_line)
                _publish_log(second, str(log_dir), "command", 0, run_line=second_line)

            self.assertFalse((log_dir / "command.prev").exists(), "an identical repeat invented an earlier run")
            self.assertIn(b"2026-01-02T00:00:00Z", (log_dir / "command.log").read_bytes())

    def test_an_older_record_without_a_run_line_compares_by_its_body(self) -> None:
        body = EXECUTED_LOG_MARKER + b"cargo output\n"
        named = EXECUTED_LOG_MARKER + b"gate: run worktree=/w tree=aaa finished=?\n" + b"cargo output\n"
        self.assertEqual(body, _body_only(body))
        self.assertEqual(body, _body_only(named))

    def test_the_published_record_names_the_run_and_its_tree(self) -> None:
        # One published directory can serve every worktree of a checkout, so a
        # verdict another worktree published has to be tellable from the reader's own.
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            tree = state("h1", changed=("crates/a/src/lib.rs",))
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")}), mock.patch(
                "gate.repo_root", return_value="/worktree-one"
            ), mock.patch("gate.read_tree_state", return_value=tree), quiet[0], quiet[1]:
                self.assertEqual(0, run_gate(["/bin/sh", "-c", "printf hello"], log_dir=str(log_dir)))

            lines = (log_dir / "command.log").read_text().splitlines()
        self.assertEqual(EXECUTED_LOG_MARKER.decode().strip(), lines[0])
        self.assertTrue(lines[1].startswith("gate: run worktree=/worktree-one tree="), lines[1])
        self.assertIn(" finished=", lines[1])
        self.assertEqual("hello", lines[2])

    def test_two_publishers_of_one_directory_wait_for_each_other(self) -> None:
        # The gate's own lock is per worktree. A published directory is not, so it
        # is the directory that decides who contends, and the lock lives in it.
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            log_dir.mkdir()
            holder = (
                "import pathlib, sys, time\n"
                f"sys.path.insert(0, {str(Path(__file__).resolve().parent)!r})\n"
                "from gate_cache import _publish_lock\n"
                f"with _publish_lock(pathlib.Path({str(log_dir)!r})):\n"
                "    print('locked', flush=True)\n"
                "    time.sleep(0.6)\n"
            )
            child = subprocess.Popen([sys.executable, "-c", holder], stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual("locked", child.stdout.readline().strip())
                started = time.monotonic()
                with _publish_lock(log_dir):
                    waited = time.monotonic() - started
            finally:
                child.wait(timeout=10)
                child.stdout.close()

            self.assertTrue((log_dir / PUBLISH_LOCK_NAME).is_file(), "the lock is not in the published directory")
            self.assertGreaterEqual(waited, 0.2, "a second publisher did not wait for the first")
