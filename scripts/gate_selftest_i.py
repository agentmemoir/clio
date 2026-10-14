from __future__ import annotations

"""The published pair: one run's log, one run's exit code, and the run named in the record."""

import contextlib
import io
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from gate import run_gate
from gate_cache import PUBLISH_LOCK_NAME, _body_only, _publish_lock, _publish_log
from gate_kinds import EXECUTED_LOG_MARKER
from gate_selftest_c import state


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
