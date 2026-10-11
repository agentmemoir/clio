"""The published log uses <kind>.log for newest and <kind>.prev for kept run."""

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from gate import main, run_gate
from gate_cache import _prune_cache
from gate_kinds import ENV_KEYS, NEXTEST_BENCH_KIND, NEXTEST_LOOKUP_KIND, KEY_SCHEME
from gate_tree import TreeState, cache_root, fingerprint, repo_key
from exec_tempdir import exec_tempdir


class CacheIdentityTests(unittest.TestCase):
    def test_same_executable_alias_shares_a_key(self):
        tree = TreeState("head", (), ())
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            first = root / "one" / "bin" / "cargo"
            alias = root / "two" / "bin" / "cargo"
            first.parent.mkdir(parents=True)
            alias.parent.mkdir(parents=True)
            first.write_text("#!/bin/sh\n# same cargo binary\n")
            first.chmod(0o755)
            alias.symlink_to(first)
            env = {"PATH": os.environ.get("PATH", "")}
            left = fingerprint(tree, env, "test", [str(first), "nextest", "run"])
            right = fingerprint(tree, env, "test", [str(alias), "nextest", "run"])
        self.assertEqual(left, right)
        left = fingerprint(tree, {}, "command", ["/one/bin/custom", "--help"])
        right = fingerprint(tree, {}, "command", ["/two/bin/custom", "--help"])
        self.assertNotEqual(left, right)

    def test_make_base_environment_changes_the_key(self):
        tree = TreeState("head", (), ())
        command = ["make", "coverage-fast"]
        left = fingerprint(tree, {"BASE": "master"}, "coverage-fast", command)
        right = fingerprint(tree, {"BASE": "origin/master"}, "coverage-fast", command)
        self.assertIn("BASE", ENV_KEYS)
        self.assertTrue(KEY_SCHEME.endswith(b"v6\0"))
        self.assertNotEqual(left, right)
        argument = fingerprint(tree, {}, "coverage-fast", [*command, "BASE=origin/master"])
        environment = fingerprint(tree, {"BASE": "origin/master"}, "coverage-fast", command)
        self.assertEqual(argument, environment)

    def test_nextest_bench_keeps_a_distinct_cached_kind(self):
        from gate_classify import command_kind

        self.assertEqual(NEXTEST_BENCH_KIND, command_kind(["cargo", "nextest", "bench"]))
        self.assertEqual(NEXTEST_BENCH_KIND, command_kind(["cargo", "nextest", "b"]))
        self.assertNotEqual(NEXTEST_LOOKUP_KIND, NEXTEST_BENCH_KIND)

    def test_lookup_entries_are_pruned_before_real_verdicts(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {"XDG_CACHE_HOME": tmp}):
            repo_cache = cache_root() / repo_key("/repo")
            kinds = ["coverage", "test", NEXTEST_LOOKUP_KIND, "llvm-cov-lookup"]
            for index, kind in enumerate(kinds):
                entry = repo_cache / str(index)
                entry.mkdir(parents=True)
                meta = entry / "meta.json"
                meta.write_text(json.dumps({"kind": kind}))
                os.utime(meta, (index + 1, index + 1))

            _prune_cache("/repo", keep=2)

            kept = [json.loads(path.read_text())["kind"] for path in repo_cache.glob("*/meta.json")]
        self.assertEqual({"coverage", "test"}, set(kept))


class DefaultLogTests(unittest.TestCase):
    def test_default_log_path_and_environment_override(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp) / "cache"
            override = Path(tmp) / "override"
            root = "/repo"
            tree = TreeState("head", (), ())
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache)}), mock.patch(
                "gate.repo_root", return_value=root
            ), mock.patch("gate.read_tree_state", return_value=tree), quiet[0], quiet[1]:
                self.assertEqual(0, run_gate(["/bin/sh", "-c", "printf default"]))
                with mock.patch.dict(os.environ, {"CLIO_GATE_LOG_DIR": str(override)}):
                    self.assertEqual(
                        0,
                        main(["run", "--", "/bin/sh", "-c", "printf override"]),
                    )

            default_log = cache / "clio-gate" / repo_key(root) / "logs" / "command.log"
            override_log = override / "command.log"
            self.assertTrue(default_log.read_text().endswith("default"))
            self.assertTrue(override_log.read_text().endswith("override"))

    def test_shim_publishes_without_a_log_directory_variable(self):
        root = Path(__file__).resolve().parent.parent
        with exec_tempdir(purpose="default gate log publication via shim") as raw:
            tmp = Path(raw)
            shimdir, realdir = tmp / "shim", tmp / "real"
            shimdir.mkdir()
            realdir.mkdir()
            stand_in = realdir / "make"
            stand_in.write_text("#!/bin/sh\necho checked\n")
            stand_in.chmod(0o755)
            (shimdir / "make").symlink_to(root / "scripts" / "gate-shim")
            env = dict(
                os.environ,
                PATH=f"{shimdir}{os.pathsep}{realdir}{os.pathsep}{os.environ.get('PATH', '')}",
                XDG_CACHE_HOME=str(tmp / "cache"),
                GATE_ACTIVE="",
                GATE_RAW="",
                MAKEFLAGS="",
                CLIO_GATE_LOG_DIR="",
            )
            proc = subprocess.run(
                [str(shimdir / "make"), "coverage"],
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            log = Path(env["XDG_CACHE_HOME"]) / "clio-gate" / repo_key(str(root)) / "logs" / "coverage.log"
            published = log.read_text()

        self.assertEqual(0, proc.returncode, proc.stderr)
        self.assertTrue(published.endswith("checked\n"))
