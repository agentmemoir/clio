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


def _competing_publisher(log_dir: str, scratch: str, letter: str, code: int, barrier) -> None:
    """Write one run's body, wait for the others, then publish it.

    The barrier is what makes the check exercise the lock: without it the publishers
    drift apart, and two of them are unlikely to be inside a publish at once.
    """
    source = Path(scratch) / f"log-{letter}"
    source.write_bytes((letter * 63 + "\n").encode() * 16)
    barrier.wait()
    _publish_log(source, log_dir, "command", code)


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

    def test_a_failed_log_write_puts_the_verdict_back(self) -> None:
        # The pair is two renames, and the verdict goes first so a failing run
        # always publishes its own code. When the log cannot follow, leaving the new
        # code in place would publish two runs, so the verdict must go back.
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            log_dir.mkdir()
            previous = EXECUTED_LOG_MARKER + b"OLD RUN\n"
            (log_dir / "command.log").write_bytes(previous)
            (log_dir / "command.exit").write_bytes(b"0\ngate: run worktree=/old tree=000 finished=?\n")
            newer = Path(tmp) / "log"
            newer.write_bytes(b"NEW FAILING RUN\n")
            # A directory stands where the log goes, so replacing the log fails.
            (log_dir / "command.log").unlink()
            (log_dir / "command.log").mkdir()
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            run_line = b"gate: run worktree=/w tree=abc finished=?\n"
            with quiet[0], quiet[1]:
                result = _publish_log(newer, str(log_dir), "command", 2, run_line=run_line)

            self.assertIsNone(result)
            self.assertEqual(
                b"0\ngate: run worktree=/old tree=000 finished=?\n",
                (log_dir / "command.exit").read_bytes(),
                "the new verdict was left beside the previous run's log",
            )

    def test_a_failed_kept_run_write_leaves_the_newest_pair(self) -> None:
        # Keeping the replaced run is the last write and the least important one.
        # Its failure must not stop the newest log, or the publish would hold two runs.
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            log_dir.mkdir()
            (log_dir / "command.log").write_bytes(EXECUTED_LOG_MARKER + b"OLD RUN\n")
            (log_dir / "command.exit").write_text("0\n")
            newer = Path(tmp) / "log"
            newer.write_bytes(b"NEW FAILING RUN\n")
            # A directory stands where the kept run goes, so that write fails.
            (log_dir / "command.prev").mkdir()
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            run_line = b"gate: run worktree=/w tree=abc finished=?\n"
            with quiet[0], quiet[1]:
                result = _publish_log(newer, str(log_dir), "command", 2, run_line=run_line)

            self.assertIsNotNone(result, "a failed kept-run write cost the newest publish")
            self.assertEqual("2", (log_dir / "command.exit").read_text().split("\n")[0].strip())
            self.assertTrue((log_dir / "command.log").read_text().endswith("NEW FAILING RUN\n"))

    def test_a_command_line_that_merely_starts_like_a_run_line_is_kept(self) -> None:
        # The run line has a shape, not just a prefix. A command whose own output
        # opens with the prefix must not have that line dropped, because an
        # identical re-run would then invent an earlier run.
        body = b"gate: run this is my own output\nrest\n"
        old_format = EXECUTED_LOG_MARKER + body
        self.assertEqual(old_format, _body_only(old_format))
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            log_dir.mkdir()
            (log_dir / "command.log").write_bytes(old_format)
            source = Path(tmp) / "log"
            source.write_bytes(body)
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with quiet[0], quiet[1]:
                _publish_log(source, str(log_dir), "command", 0, run_line=b"gate: run worktree=/w tree=abc finished=?\n")

            self.assertFalse((log_dir / "command.prev").exists(), "an identical body invented an earlier run")

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
            exit_lines = (log_dir / "command.exit").read_text().splitlines()
        self.assertEqual(EXECUTED_LOG_MARKER.decode().strip(), lines[0])
        self.assertTrue(lines[1].startswith("gate: run worktree=/worktree-one tree="), lines[1])
        self.assertIn(" finished=", lines[1])
        self.assertEqual("hello", lines[2])
        self.assertEqual("0", exit_lines[0], "the exit file's first line is not the exit code")
        self.assertEqual(lines[1], exit_lines[1], "the exit file does not name the run the log names")

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

    def test_a_replay_names_the_run_whose_verdict_it_is(self) -> None:
        # The first observation in issue #186 is a published replay. A reader has to
        # be able to see that the verdict belongs to a different run, so the replay
        # must name the run it replays, not the replay itself.
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp) / "logs"
            tree = state("h1", changed=("crates/a/src/lib.rs",))
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")}), mock.patch(
                "gate.repo_root", return_value="/worktree-one"
            ), mock.patch("gate.read_tree_state", return_value=tree), quiet[0], quiet[1]:
                self.assertEqual(0, run_gate(["/bin/sh", "-c", "printf hello"], log_dir=str(log_dir)))
                first = (log_dir / "command.exit").read_text().splitlines()
                self.assertEqual(0, run_gate(["/bin/sh", "-c", "printf hello"], log_dir=str(log_dir)))
                replayed = (log_dir / "command.exit").read_text().splitlines()

            self.assertIn("cached verdict replay", (log_dir / "command.log").read_text().splitlines()[0])
        self.assertEqual(2, len(first), "the first run's exit file does not name its run")
        self.assertTrue(first[1].startswith("gate: run worktree=/worktree-one tree="), first[1])
        self.assertEqual(first, replayed, "the replay did not name the run whose verdict it published")

    def test_competing_publishers_leave_one_run_and_no_staging_files(self) -> None:
        # Serialization is the whole fix, and a check that only ever published one
        # run could not exercise it. Four processes publish at once, and afterwards
        # the directory must hold one run's pair and none of their staging files.
        import multiprocessing as mp

        if "fork" not in mp.get_all_start_methods():
            self.skipTest(f"no fork start method in {mp.get_all_start_methods()}")
        codes = {"A": 2, "B": 0}
        letters = "ABAB"
        with tempfile.TemporaryDirectory() as tmp:
            for round_index in range(3):
                log_dir = Path(tmp) / f"logs-{round_index}"
                log_dir.mkdir()
                scratch = Path(tmp) / f"scratch-{round_index}"
                scratch.mkdir()
                context = mp.get_context("fork")
                barrier = context.Barrier(len(letters))
                workers = [
                    context.Process(
                        target=_competing_publisher,
                        args=(str(log_dir), str(scratch), letter, codes[letter], barrier),
                    )
                    for letter in letters
                ]
                for worker in workers:
                    worker.start()
                for worker in workers:
                    worker.join(timeout=30)
                    self.assertFalse(worker.is_alive(), "a publisher did not finish")

                published = [line for line in (log_dir / "command.log").read_text().splitlines() if line]
                body = published[-1][:1] if published else ""
                code = (log_dir / "command.exit").read_text().splitlines()[0].strip()

                self.assertIn(body, codes, f"round {round_index}: the log holds no run's body")
                self.assertEqual(
                    str(codes[body]),
                    code,
                    f"round {round_index}: the exit code and the log describe different runs",
                )
                self.assertEqual(
                    [],
                    list(log_dir.glob(".*.staged")),
                    f"round {round_index}: a staging file was left in the published directory",
                )
