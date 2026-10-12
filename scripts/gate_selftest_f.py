"""Gate cache identity, work evidence, and direct dry-run checks."""

from __future__ import annotations

import contextlib
import io
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from exec_tempdir import exec_tempdir
from gate import main, run_gate
from gate_classify import command_kind, make_is_dry_run
from gate_kinds import ENV_KEYS, EXECUTED_LOG_MARKER, KEY_SCHEME, REPLAYED_LOG_MARKER
from gate_tree import TreeState, fingerprint
from gate_selftest_c import _is_test_surface_file


class MakeDryRunTests(unittest.TestCase):
    def test_make_flag_spellings_and_makeflags_values(self) -> None:
        rows = (
            (["make", "-n", "coverage"], {}, True),
            (["make", "--dry-run", "coverage"], {}, True),
            (["make", "--dry", "coverage"], {}, True),
            (["make", "-ns", "coverage"], {}, True),
            (["make", "coverage", "-n"], {}, True),
            (["make", "coverage"], {"MAKEFLAGS": "n"}, True),
            (["make", "coverage"], {"MAKEFLAGS": "ksn"}, True),
            (["make", "coverage"], {"MAKEFLAGS": "PACKAGES=a n"}, True),
            (["make", "coverage"], {"GNUMAKEFLAGS": "--recon"}, True),
            (["make", "-f", "-n", "coverage"], {}, False),
            (["make", "--file", "-n", "coverage"], {}, False),
            (["make", "--assume-old", "-n", "coverage"], {}, False),
            (["make", "--", "-n", "coverage"], {}, False),
            (["make", "coverage"], {"MAKEFLAGS": "PACKAGES=-n"}, False),
            (["make", "coverage"], {"MAKEFLAGS": "--output-sync=target"}, False),
            (["cargo", "clippy"], {"MAKEFLAGS": "n"}, False),
        )
        for argv, env, expected in rows:
            with self.subTest(argv=argv, env=env):
                self.assertEqual(expected, make_is_dry_run(argv, env))

    def test_direct_gate_dry_runs_do_not_lock_cache_or_publish(self) -> None:
        rows = (
            (["-n", "coverage"], {}),
            (["coverage"], {"MAKEFLAGS": "n"}),
            (["coverage"], {"GNUMAKEFLAGS": "-n"}),
        )
        with tempfile.TemporaryDirectory() as scratch:
            for index, (args, flags) in enumerate(rows):
                cache = Path(scratch) / f"cache-{index}"
                logs = Path(scratch) / f"logs-{index}"
                env = {
                    "XDG_CACHE_HOME": str(cache),
                    "CLIO_GATE_LOG_DIR": str(logs),
                    "MAKEFLAGS": "",
                    "GNUMAKEFLAGS": "",
                    **flags,
                }
                with self.subTest(args=args, flags=flags), mock.patch.dict(os.environ, env):
                    with mock.patch("gate.repo_root", side_effect=AssertionError("dry run reached repo scan")) as root, mock.patch(
                        "gate.subprocess.call", return_value=19
                    ) as child, contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                        code = main(["run", "--", "make", *args])
                    self.assertEqual(19, code)
                    root.assert_not_called()
                    self.assertEqual(["make", *args], child.call_args.args[0])
                    self.assertEqual("1", child.call_args.kwargs["env"]["GATE_ACTIVE"])
                    self.assertFalse((cache / "clio-gate").exists())
                    self.assertFalse(logs.exists())

    def test_installed_shim_passes_makeflags_dry_run_without_publication(self) -> None:
        root = Path(__file__).resolve().parent.parent
        for name, value in (("MAKEFLAGS", "n"), ("GNUMAKEFLAGS", "-n")):
            with self.subTest(name=name), exec_tempdir(purpose="make dry-run through the gate shim") as raw:
                tmp = Path(raw)
                shimdir, realdir = tmp / "shim", tmp / "real"
                shimdir.mkdir()
                realdir.mkdir()
                stand_in = realdir / "make"
                stand_in.write_text("#!/bin/sh\nprintf 'dry-run recipe\\n'\n")
                stand_in.chmod(0o755)
                (shimdir / "make").symlink_to(root / "scripts" / "gate-shim")
                cache, logs = tmp / "cache", tmp / "logs"
                env = dict(
                    os.environ,
                    PATH=f"{shimdir}{os.pathsep}{realdir}{os.pathsep}{os.environ.get('PATH', '')}",
                    XDG_CACHE_HOME=str(cache),
                    CLIO_GATE_LOG_DIR=str(logs),
                    GATE_ACTIVE="",
                    GATE_RAW="",
                    MAKEFLAGS=value if name == "MAKEFLAGS" else "",
                    GNUMAKEFLAGS=value if name == "GNUMAKEFLAGS" else "",
                )
                proc = subprocess.run(
                    [str(shimdir / "make"), "coverage"],
                    cwd=root,
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(0, proc.returncode, proc.stderr)
                self.assertEqual("dry-run recipe\n", proc.stdout)
                self.assertFalse((cache / "clio-gate").exists())
                self.assertFalse(logs.exists())


class CacheIdentityTests(unittest.TestCase):
    @mock.patch("gate_tree._rustup_toolchain_identity", return_value="rustup")
    def test_tool_aliases_share_content_identity_but_distinct_binaries_do_not(self, _rustup) -> None:
        tree = TreeState("head", (), ())
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            first = tmp / "first" / "cargo"
            alias = tmp / "alias" / "cargo"
            different = tmp / "different" / "cargo"
            for path, content in (
                (first, "#!/bin/sh\n# cargo-version-one\n"),
                (different, "#!/bin/sh\n# cargo-version-two\n"),
            ):
                path.parent.mkdir(parents=True)
                path.write_text(content)
                path.chmod(0o755)
            alias.parent.mkdir()
            alias.symlink_to(first)
            env = {"PATH": os.environ.get("PATH", "")}
            direct = fingerprint(tree, env, "test", [str(first), "nextest", "b"])
            same_tool = fingerprint(tree, env, "test", [str(alias), "nextest", "bench"])
            new_tool = fingerprint(tree, env, "test", [str(different), "nextest", "bench"])

        self.assertEqual(direct, same_tool)
        self.assertNotEqual(direct, new_tool)
        self.assertTrue(KEY_SCHEME.endswith(b"v6\0"))

    def test_make_aliases_share_content_identity_but_distinct_binaries_do_not(self) -> None:
        tree = TreeState("head", (), ())
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            first = tmp / "first" / "make"
            alias = tmp / "alias" / "make"
            different = tmp / "different" / "make"
            for path, content in (
                (first, b"make version one"),
                (different, b"make version two"),
            ):
                path.parent.mkdir(parents=True)
                path.write_bytes(content)
                path.chmod(0o755)
            alias.parent.mkdir()
            alias.symlink_to(first)
            env = {"PATH": os.environ.get("PATH", "")}
            with mock.patch("gate_tree._rustup_toolchain_identity", return_value="rustup"), mock.patch(
                "gate_tree._cargo_config_identity", return_value="config"
            ):
                first_key = fingerprint(tree, env, "coverage", [str(first), "coverage"])
                alias_key = fingerprint(tree, env, "coverage", [str(alias), "coverage"])
                different_key = fingerprint(tree, env, "coverage", [str(different), "coverage"])

        self.assertEqual(first_key, alias_key)
        self.assertNotEqual(first_key, different_key)

    def test_rustup_selected_toolchain_binaries_change_the_key(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            proxy = tmp / "cargo-proxy"
            rustup = tmp / "rustup"
            cargo = tmp / "toolchain" / "bin" / "cargo"
            rustc = tmp / "toolchain" / "bin" / "rustc"
            for path, content in (
                (proxy, b"proxy"),
                (rustup, b"rustup"),
                (cargo, b"cargo version one"),
                (rustc, b"rustc version one"),
            ):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
            tool_paths = {
                "cargo": proxy,
                "rustup": rustup,
                str(rustup): rustup,
                str(cargo): cargo,
                str(rustc): rustc,
            }

            def which(command, path=None):
                if command in tool_paths:
                    return str(tool_paths[command])
                candidate = Path(command)
                return str(candidate) if candidate.is_file() else None

            resolved: list[list[str]] = []

            def run(command, **_kwargs):
                resolved.append(command)
                selected = cargo if command[-1] == "cargo" else rustc
                return subprocess.CompletedProcess(command, 0, stdout=f"{selected}\n".encode())

            env = {"PATH": os.environ.get("PATH", "")}
            with mock.patch("gate_tree.shutil.which", side_effect=which), mock.patch(
                "gate_tree.subprocess.run", side_effect=run
            ):
                first = fingerprint(
                    TreeState("head", (), ()), env, "clippy", ["cargo", "+nightly", "clippy"]
                )
                rustc.write_bytes(b"rustc version two")
                second = fingerprint(
                    TreeState("head", (), ()), env, "clippy", ["cargo", "+nightly", "clippy"]
                )

        self.assertNotEqual(first, second)
        self.assertTrue(all(command[-3:-1] == ["--toolchain", "nightly"] for command in resolved))

    @mock.patch("gate_tree._rustup_toolchain_identity", return_value="rustup")
    def test_result_changing_environment_values_separate_cache_keys(self, _rustup) -> None:
        tree = TreeState("head", (), ())
        base = {"PATH": os.environ.get("PATH", "")}
        command = ["cargo", "nextest", "run"]
        for name in ("PATH", "CARGO_HOME", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER"):
            with self.subTest(name=name):
                changed = dict(base, **{name: "different"})
                self.assertIn(name, ENV_KEYS)
                self.assertNotEqual(
                    fingerprint(tree, base, "test", command),
                    fingerprint(tree, changed, "test", command),
                )

    @mock.patch("gate_tree._rustup_toolchain_identity", return_value="rustup")
    def test_cargo_configuration_content_separates_cache_keys(self, _rustup) -> None:
        tree = TreeState("head", (), ())
        with tempfile.TemporaryDirectory() as raw:
            cargo_home = Path(raw) / "cargo"
            cargo_home.mkdir()
            config = cargo_home / "config.toml"
            env = {"PATH": os.environ.get("PATH", ""), "CARGO_HOME": str(cargo_home)}
            config.write_text("[build]\nrustflags = ['-Dwarnings']\n")
            first = fingerprint(tree, env, "clippy", ["cargo", "clippy"])
            config.write_text("[build]\nrustflags = ['-Dwarnings', '-Cdebuginfo=0']\n")
            second = fingerprint(tree, env, "clippy", ["cargo", "clippy"])
        self.assertNotEqual(first, second)
        dynamic = dict(env, CARGO_PROFILE_TEST_LTO="true")
        self.assertNotEqual(
            fingerprint(tree, env, "test", ["cargo", "test"]),
            fingerprint(tree, dynamic, "test", ["cargo", "test"]),
        )

    @mock.patch("gate_tree._rustup_toolchain_identity", return_value="rustup")
    def test_make_target_alias_and_base_spellings_share_keys(self, _rustup) -> None:
        tree = TreeState("head", (), ())
        env = {"PATH": os.environ.get("PATH", ""), "BASE": "origin/master"}
        alias = fingerprint(tree, env, "coverage", ["make", "cov"])
        target = fingerprint(tree, env, "coverage", ["make", "coverage"])
        option_alias = fingerprint(tree, env, "coverage", ["make", "-j4", "cov"])
        option_target = fingerprint(tree, env, "coverage", ["make", "-j4", "coverage"])
        file_name = fingerprint(tree, env, "coverage", ["make", "-f", "cov", "coverage"])
        different_file = fingerprint(tree, env, "coverage", ["make", "-f", "coverage", "coverage"])
        base_argument = fingerprint(
            tree,
            {"PATH": env["PATH"]},
            "coverage-fast",
            ["make", "coverage-fast", "BASE=origin/master"],
        )
        base_environment = fingerprint(tree, env, "coverage-fast", ["make", "coverage-fast"])
        self.assertEqual(alias, target)
        self.assertEqual(option_alias, option_target)
        self.assertEqual(
            fingerprint(tree, env, "coverage", ["make", "cov", "cov"]),
            fingerprint(tree, env, "coverage", ["make", "coverage", "coverage"]),
        )
        self.assertNotEqual(file_name, different_file)
        self.assertEqual("coverage", command_kind(["make", "-f", "cov", "coverage"]))
        self.assertEqual("make", command_kind(["make", "--assume-old", "coverage", "help"]))
        self.assertEqual(base_argument, base_environment)


class WorkEvidenceTests(unittest.TestCase):
    def test_fresh_and_replayed_clippy_logs_report_observed_package_work(self) -> None:
        samples = (
            ("Finished `dev` profile [unoptimized] target(s) in 0.00s\n", b"0 checked, 0 compiled"),
            (
                "Checking clio v0.1.0 (/repo)\n"
                "Checking clio-types v0.1.0 (/repo)\n"
                "Compiling clio-build v0.1.0 (/repo)\n"
                "Finished `dev` profile [unoptimized] target(s) in 0.00s\n",
                b"2 checked, 1 compiled",
            ),
            ("quiet cargo output\n", b"unavailable (no status lines)"),
        )
        for index, (output, work) in enumerate(samples):
            with self.subTest(work=work), tempfile.TemporaryDirectory() as raw:
                tmp = Path(raw)
                cache, logs = tmp / "cache", tmp / "logs"
                tree = TreeState("head", (), ())
                with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache), "PATH": os.environ.get("PATH", ""), "CLIO_GATE_LOG_DIR": ""}), mock.patch(
                    "gate.repo_root", return_value=str(tmp)
                ), mock.patch("gate.read_tree_state", return_value=tree), mock.patch(
                    "gate.scope_command", side_effect=lambda _kind, argv, _root, _changed: (argv, [])
                ), mock.patch("gate.subprocess.call", side_effect=lambda _argv, **kwargs: _write_output(kwargs["stdout"], output)) as child:
                    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                        self.assertEqual(0, run_gate(["cargo", "clippy"], log_dir=str(logs)))
                        fresh = (logs / "clippy.log").read_bytes()
                        self.assertEqual(0, run_gate(["cargo", "clippy"], log_dir=str(logs)))
                        replay = (logs / "clippy.log").read_bytes()
                self.assertEqual(1, child.call_count)
                self.assertTrue(fresh.startswith(EXECUTED_LOG_MARKER))
                self.assertIn(b"Cargo package work: " + work, fresh)
                self.assertTrue(replay.startswith(REPLAYED_LOG_MARKER))
                self.assertIn(b"Cargo package work: " + work, replay)


def _write_output(stdout, output: str) -> int:
    stdout.write(output.encode())
    return 0


class SurfaceInventoryTests(unittest.TestCase):
    def test_log_surface_scan_skips_test_code_but_keeps_product_sources(self) -> None:
        self.assertTrue(_is_test_surface_file("scripts/gate_selftest_f.py"))
        self.assertTrue(_is_test_surface_file("scripts/gate_self_test_support.py"))
        self.assertTrue(_is_test_surface_file("scripts/precommit_testkit.py"))
        self.assertTrue(_is_test_surface_file("crates/clio/tests/gate.py"))
        self.assertTrue(_is_test_surface_file("scripts/check_test_surface.sh"))
        self.assertTrue(_is_test_surface_file("crates/clio/src/test_fixture.rs"))
        self.assertFalse(_is_test_surface_file("scripts/gate.py"))
        self.assertFalse(_is_test_surface_file("docs/gate-logs.md"))
