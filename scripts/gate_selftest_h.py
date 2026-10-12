"""A command that must run every time is never replayed and never stores a verdict."""

from __future__ import annotations

import contextlib
import io
import os
import unittest
from pathlib import Path
from unittest import mock

from exec_tempdir import exec_tempdir
from gate import *  # noqa: F403 - re-exports every engine name the tests reference

def state(head: str, contents=(), changed=()) -> TreeState:
    return TreeState(head, tuple(contents), tuple(changed))

class Tests(unittest.TestCase):
    def test_a_clean_never_replays(self):
        # `clean` removes build output, so a replayed verdict would report
        # success without having removed anything. Both spellings are named
        # as a command rather than as a target, because that is how the tool
        # receives it.
        for argv in (
            ["make", "coverage-clean"],
            ["cargo", "llvm-cov", "clean"],
            ["cargo", "llvm-cov", "clean", "--workspace"],
        ):
            self.assertNotEqual("coverage", command_kind(argv))
        with exec_tempdir(purpose="a stand-in cargo command") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "cargo"
            stand_in.write_text(f'#!/bin/sh\necho run >> "{counter}"\nexit 5\n')
            stand_in.chmod(0o755)
            clean = [str(stand_in), "llvm-cov", "clean"]
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("gate.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "gate.read_tree_state",
                return_value=state("h1", contents=(("crates/a/src/lib.rs", "clean"),), changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                run_gate(clean)
                run_gate(clean)
                self.assertEqual(2, counter.read_text().count("run"))

    def test_a_capture_never_replays(self):
        # The golden capture freezes the CLI fixture from a guarded release
        # build, so whether it can proceed depends on the release build cache,
        # which is not part of the fingerprint. A stored refusal would replay
        # after the cache was warmed, which is the step its own refusal message
        # asks for -- the defect #136 records -- and a stored success would let
        # a later run report a capture it never made. The capture therefore
        # runs every time, and it stores nothing, so a run of the same command
        # without the capture switch cannot inherit its verdict either.
        with exec_tempdir(purpose="a stand-in capture command") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "cargo"
            stand_in.write_text(f'#!/bin/sh\necho run >> "{counter}"\nexit 5\n')
            stand_in.chmod(0o755)
            capture = [
                str(stand_in),
                "nextest",
                "run",
                "-p",
                "clio",
                "--test",
                "golden_repeatability_test",
                "-E",
                "test(capture_mode_writes_the_fixture)",
            ]
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("gate.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "gate.read_tree_state",
                return_value=state(
                    "h1",
                    contents=(("crates/a/src/lib.rs", "capture"),),
                    changed=("crates/a/src/lib.rs",),
                ),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                with mock.patch.dict(os.environ, {"CLIO_GOLDEN_CAPTURE": "1"}):
                    run_gate(capture)
                    run_gate(capture)
                self.assertEqual(
                    2,
                    counter.read_text().count("run"),
                    "a capture must run on the second invocation too, not replay a stored verdict",
                )
                with mock.patch.dict(os.environ, {"CLIO_GOLDEN_CAPTURE": ""}):
                    run_gate(capture)
                self.assertEqual(
                    3,
                    counter.read_text().count("run"),
                    "a capture must store no verdict for a later run to replay",
                )
