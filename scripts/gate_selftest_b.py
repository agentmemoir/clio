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

from exec_tempdir import exec_tempdir
from gate import *  # noqa: F403 - re-exports every engine name the tests reference
from gate import _artifact_rel_paths, _purge_coverage_cache, _restore_artifacts, _save_artifacts
from gate_kinds import NEXTEST_BENCH_KIND, NEXTEST_BENCH_SUBCOMMANDS

def state(head: str, contents=(), changed=()) -> TreeState:
    return TreeState(head, tuple(contents), tuple(changed))

members = [
    Member("a", "/repo/crates/a", ("b",)),
    Member("b", "/repo/crates/b", ()),
    Member("c", "/repo/crates/c", ()),
]

def git(root: Path, *args: str) -> None:
    """Run git in `root` under a fixed identity, failing on error."""
    subprocess.run(
        ["git", *args],
        cwd=root,
        check=True,
        capture_output=True,
        env=dict(
            os.environ,
            GIT_AUTHOR_NAME="t",
            GIT_AUTHOR_EMAIL="t@t",
            GIT_COMMITTER_NAME="t",
            GIT_COMMITTER_EMAIL="t@t",
        ),
    )

def git_repo(root: Path) -> None:
    """Init `root` as a repo with one committed file, so `HEAD` exists."""
    git(root, "init", "-q", "-b", "master")
    (root / "tracked.txt").write_text("one\n")
    git(root, "add", "tracked.txt")
    git(root, "commit", "-qm", "init")

def key(root: Path) -> str:
    """The gate fingerprint of the real git tree at `root`."""
    return fingerprint(read_tree_state(str(root)), {}, "test", ["cargo", "test"])

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

    def test_staging_an_added_file_keeps_the_fingerprint(self):
        # The key describes the working tree, not the index. Before this, an
        # added file reached the key as a hashed untracked entry and then,
        # after `git add`, inside the diff text, so the same bytes keyed two
        # entries and a verdict measured before staging did not replay.
        with exec_tempdir(purpose="a staged added file") as tmp:
            root = Path(tmp)
            git_repo(root)
            (root / "tracked.txt").write_text("one\ntwo\n")
            (root / "added.rs").write_text("// new\n")
            before = key(root)
            git(root, "add", "added.rs", "tracked.txt")
            self.assertEqual(before, key(root), "staging does not change the working-tree key")

    def test_a_mode_change_on_a_modified_file_moves_the_key(self):
        # The old diff-text key carried the `old mode`/`new mode` lines; a
        # content-only key would not, so the executable bit is hashed too.
        with exec_tempdir(purpose="a mode change") as tmp:
            root = Path(tmp)
            git_repo(root)
            (root / "tracked.txt").write_text("one\ntwo\n")
            before = key(root)
            (root / "tracked.txt").chmod(0o755)
            self.assertNotEqual(before, key(root), "a mode change moves the key")

    def test_deleting_a_tracked_file_moves_the_key(self):
        with exec_tempdir(purpose="a deleted file") as tmp:
            root = Path(tmp)
            git_repo(root)
            before = key(root)
            (root / "tracked.txt").unlink()
            self.assertNotEqual(before, key(root), "a deletion moves the key")

    def test_the_two_lists_of_nextest_subcommands_are_one_list(self):
        # Narrowing a command and classifying it are two decisions about the
        # same fact, and they used to be written down twice. A subcommand in
        # one list and not the other is a command cached as a test but never
        # narrowed, or narrowed under a kind that says it is not one.
        self.assertTrue(NEXTEST_EXECUTING_SUBCOMMANDS)
        self.assertTrue(NEXTEST_EXECUTING_SUBCOMMANDS <= NEXTEST_KNOWN_SUBCOMMANDS)
        for sub in sorted(NEXTEST_EXECUTING_SUBCOMMANDS):
            self.assertEqual("test", command_kind(["cargo", "nextest", sub]))
            with mock.patch("gate.workspace_members", return_value=members):
                scoped, _ = scope_command(
                    "test", ["cargo", "nextest", sub, "--workspace"], "/repo", ("crates/b/src/lib.rs",)
                )
            self.assertIn("-p", scoped, f"{sub} runs tests and should be narrowed")
        for sub in sorted(NEXTEST_KNOWN_SUBCOMMANDS - NEXTEST_EXECUTING_SUBCOMMANDS - NEXTEST_BENCH_SUBCOMMANDS):
            self.assertEqual(NEXTEST_LOOKUP_KIND, command_kind(["cargo", "nextest", sub]), sub)
            self.assertEqual(
                (["cargo", "nextest", sub, "--workspace"], None),
                scope_command(
                    NEXTEST_LOOKUP_KIND,
                    ["cargo", "nextest", sub, "--workspace"],
                    "/repo",
                    ("crates/b/src/lib.rs",),
                ),
                sub,
            )
        for sub in sorted(NEXTEST_BENCH_SUBCOMMANDS):
            argv = ["cargo", "nextest", sub, "--workspace"]
            self.assertEqual(NEXTEST_BENCH_KIND, command_kind(argv), sub)
            self.assertEqual((argv, None), scope_command(NEXTEST_BENCH_KIND, argv, "/repo", ("crates/b/src/lib.rs",)))

    def test_the_package_flag_lands_after_an_option_led_subcommand(self):
        # The bug this shape caused: a fixed index put the package flag in
        # front of the subcommand, and `cargo nextest -p clio list` is not a
        # legal command, so the hook that runs the listing reported a
        # failure carrying an error from a command nobody had asked for.
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, scope = scope_command(
                "test", ["cargo", "nextest", "--verbose", "run", "--workspace"], "/repo", ("crates/b/src/lib.rs",)
            )
        self.assertEqual(["cargo", "nextest", "--verbose", "run", "-p", "a", "-p", "b"], scoped)
        self.assertEqual(["a", "b"], scope)
        with mock.patch("gate.workspace_members", return_value=members):
            aliased, _ = scope_command(
                "test", ["cargo", "nextest", "-P", "ci", "r", "--workspace"], "/repo", ("crates/b/src/lib.rs",)
            )
        self.assertEqual(["cargo", "nextest", "-P", "ci", "r", "-p", "a", "-p", "b"], aliased)

    def test_every_coverage_kind_is_registered_as_one(self):
        # A kind that runs the coverage report but is missing from this set
        # would not save its artifact and would survive a clean, which is
        # the report the next run compares against.
        for target, kind in sorted(MAKE_KINDS.items()):
            if "coverage" in target or "cov" == target:
                self.assertIn(kind, COVERAGE_KINDS, f"{target} publishes as {kind}, which is not registered")
        self.assertIn("coverage", COVERAGE_KINDS)
        self.assertIn("coverage-fast", COVERAGE_KINDS)
        # A clean is recognised by the target it was asked for, because the
        # kind no longer says "coverage".
        self.assertEqual("coverage-clean", command_kind(["make", "coverage-clean"]))
        self.assertEqual("coverage-html", command_kind(["make", "coverage-html"]))
        self.assertEqual("coverage", command_kind(["make", "coverage"]))
        self.assertEqual("coverage", command_kind(["make", "cov"]))
        for derived in ("coverage-html", "coverage-lcov", "coverage-open", "coverage-clean"):
            self.assertNotEqual(
                "coverage", command_kind(["make", derived]), f"{derived} would overwrite the coverage log"
            )
        # Coverage is never narrowed, whatever its kind.
        for kind in sorted(COVERAGE_KINDS):
            argv = ["make", kind]
            self.assertEqual((argv, None), scope_command(kind, argv, "/repo", ("crates/b/src/lib.rs",)))

    def test_a_coverage_kind_saves_one_named_path(self):
        # What the comment above COVERAGE_KINDS has to describe, checked here
        # rather than left as a sentence a reader has to take on trust. A make
        # target's argument vector never carries `--output-path`: the recipe
        # that would is inside the Makefile, and this wrapper is handed the
        # target name. So every coverage kind falls back to the single path
        # named below, and only `make coverage` writes it. A kind that does not
        # write that path is registered for the verdict, not for an artifact.
        for target in sorted(MAKE_KINDS):
            if MAKE_KINDS[target] not in COVERAGE_KINDS:
                continue
            self.assertEqual(
                [COVERAGE_ARTIFACT],
                _artifact_rel_paths(["make", target], MAKE_KINDS[target]),
                f"{target} saves an artifact other than the one named here",
            )
        # The half that does work: a command that names its own output path.
        # A tool called directly gets that path read out of the command line.
        self.assertEqual(
            ["reports/out.json"],
            _artifact_rel_paths(["cargo", "llvm-cov", "report", "--output-path", "reports/out.json"], "report"),
        )

    def test_a_replay_puts_back_the_artifact_it_saved(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            entry = Path(tmp) / "entry"
            report = Path(root) / COVERAGE_ARTIFACT
            report.parent.mkdir(parents=True)
            report.write_bytes(b'{"run": "first"}')
            saved = _save_artifacts(entry, str(root), ["make", "coverage"], "coverage")
            self.assertEqual([COVERAGE_ARTIFACT], saved)
            report.unlink()
            with contextlib.redirect_stdout(io.StringIO()):
                _restore_artifacts(entry, str(root), {"artifacts": saved})
            self.assertEqual(b'{"run": "first"}', report.read_bytes())

    def test_a_replay_leaves_an_artifact_written_since_it_alone(self):
        # A report is under `target/`, so it is not in the tree fingerprint and
        # a replay can meet one that is newer than the copy it holds. Writing
        # the older copy back would leave the reader with a measurement of a
        # tree that is not this one, and nothing in the log to say so. The
        # fingerprint names the source, never the build output.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            entry = Path(tmp) / "entry"
            report = Path(root) / COVERAGE_ARTIFACT
            report.parent.mkdir(parents=True)
            report.write_bytes(b'{"run": "cached"}')
            saved = _save_artifacts(entry, str(root), ["make", "coverage"], "coverage")
            report.write_bytes(b'{"run": "newer"}')
            with contextlib.redirect_stdout(io.StringIO()):
                _restore_artifacts(entry, str(root), {"artifacts": saved})
            self.assertEqual(b'{"run": "newer"}', report.read_bytes())

    def test_a_replay_replaces_an_emptied_artifact(self):
        # An empty file is what a shell redirect or an interrupted write leaves
        # behind, and it holds no measurement, so restoring over it is a repair
        # rather than a loss.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            entry = Path(tmp) / "entry"
            report = Path(root) / COVERAGE_ARTIFACT
            report.parent.mkdir(parents=True)
            report.write_bytes(b'{"run": "cached"}')
            saved = _save_artifacts(entry, str(root), ["make", "coverage"], "coverage")
            report.write_bytes(b"")
            with contextlib.redirect_stdout(io.StringIO()):
                _restore_artifacts(entry, str(root), {"artifacts": saved})
            self.assertEqual(b'{"run": "cached"}', report.read_bytes())

    def test_changed_seeds_and_root_file(self):
        self.assertEqual({"b"}, changed_seeds("/repo", ("crates/b/src/lib.rs",), members))
        # A root file that no crate compiles against is ignored for scoping,
        # so the seed set stays empty instead of forcing a workspace-wide run.
        self.assertEqual(set(), changed_seeds("/repo", ("README.md",), members))
        # A root file that does reach every crate is still workspace-wide.
        self.assertIsNone(changed_seeds("/repo", ("Cargo.toml",), members))
        self.assertEqual(set(), changed_seeds("/repo", (), members))

    def test_closure_follows_reverse_dependencies(self):
        self.assertEqual(["a", "b"], closure({"b"}, members))
        self.assertEqual(["c"], closure({"c"}, members))

    def test_scope_command_rewrites_workspace(self):
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, scope = scope_command("test", ["cargo", "test", "--locked", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(["cargo", "test", "-p", "a", "-p", "b", "--locked"], scoped)
        self.assertEqual(["a", "b"], scope)

    def test_scope_command_keeps_flags_before_double_dash(self):
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, _ = scope_command("test", ["cargo", "test", "--locked", "--", "--nocapture"], "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(["cargo", "test", "-p", "a", "-p", "b", "--locked", "--", "--nocapture"], scoped)
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, _ = scope_command("test", ["cargo", "nextest", "run", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(["cargo", "nextest", "run", "-p", "a", "-p", "b"], scoped)

    def test_a_lint_run_that_named_the_workspace_is_not_narrowed(self):
        # The command the pull request template prints, run by a contributor
        # on a machine where the shim is installed and the tree is dirty. The
        # wrapper used to strip `--workspace` and substitute the changed
        # crates, so a run the checklist described as workspace-wide measured
        # a handful of crates and published the result as the lint verdict.
        # `make lint` stays scoped: that is a different path, through
        # PACKAGES, and it is scoped on purpose.
        command = ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, scope = scope_command("clippy", command, "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(command, scoped)
        self.assertIsNone(scope)

    def test_an_unstated_lint_run_is_still_narrowed(self):
        # The exemption is for a run that stated its scope. One that did not
        # is narrowed as before, or the fast local lint disappears with it.
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, scope = scope_command("clippy", ["cargo", "clippy", "--all-targets"], "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(["cargo", "clippy", "-p", "a", "-p", "b", "--all-targets"], scoped)
        self.assertEqual(["a", "b"], scope)

    def test_a_lint_run_that_named_one_crate_is_still_left_alone(self):
        command = ["cargo", "clippy", "-p", "b", "--all-targets"]
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, scope = scope_command("clippy", command, "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(command, scoped)
        self.assertIsNone(scope)

    def test_scope_leaves_unmodelled_nextest_subcommands_alone(self):
        # The exact command the pre-commit hook runs. Narrowing it broke the
        # hook, because a package flag before the subcommand is not a legal
        # command at all and nextest refused to run any of it.
        #
        # The kind comes from the classifier rather than a literal, so this
        # keeps testing what production does with the command instead of
        # what it did when the two were written down by hand.
        argv = ["cargo", "nextest", "list", "--workspace", "--locked"]
        self.assertEqual(NEXTEST_LOOKUP_KIND, command_kind(argv))
        self.assertEqual(
            (argv, None), scope_command(command_kind(argv), argv, "/repo", ("crates/b/src/lib.rs",))
        )
        for sub in ("show-config", "archive", "bench", "self", "help"):
            other = ["cargo", "nextest", sub, "--workspace"]
            self.assertEqual(
                (other, None), scope_command(command_kind(other), other, "/repo", ("crates/b/src/lib.rs",))
            )

    def test_scope_places_the_flag_after_the_nextest_subcommand(self):
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, _ = scope_command("test", ["cargo", "nextest", "run", "--locked", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(["cargo", "nextest", "run", "-p", "a", "-p", "b", "--locked"], scoped)
        with mock.patch("gate.workspace_members", return_value=members):
            aliased, _ = scope_command("test", ["cargo", "nextest", "r", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(["cargo", "nextest", "r", "-p", "a", "-p", "b"], aliased)

    def test_the_cheap_local_gate_is_cached_and_never_narrowed(self):
        # `make ritual` must publish under its own kind so the procedure
        # files can read logs/ritual.log, and must NOT be narrowed: its clippy
        # is workspace-wide on purpose, because a crate the change did not
        # touch can still fail to compile against a signature it changed.
        self.assertEqual("ritual", command_kind(["make", "ritual"]))
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, scope = scope_command("ritual", ["make", "ritual"], "/repo", ("crates/b/src/lib.rs",))
        self.assertIsNone(scope)
        self.assertEqual(["make", "ritual"], scoped)

    def test_a_renamed_gate_target_stays_in_the_shim_dispatch_list(self):
        # The wrapper's coverage of a make target lives in two places: the
        # shim's dispatch list, which decides whether the call is gated at
        # all, and MAKE_KINDS, which decides the published kind name. A target
        # missing from the first is never cached, never locked and never
        # published, which is how `verify` sat outside the wrapper for its
        # whole life while three files claimed it was inside it.
        #
        # The dispatch list is the `heavy=1` case arm inside the loop over
        # `"$@"`. Parsing it as prose is what made an earlier version of this
        # test pass against a shim that no longer listed the target, so this
        # one only accepts a target that appears on the arm that sets heavy=1.
        shim = Path(__file__).resolve().parent / "gate-shim"
        text = shim.read_text()
        lines = text.splitlines()
        heavy_at = [i for i, line in enumerate(lines) if "heavy=1" in line]
        self.assertTrue(heavy_at, "the shim no longer marks any command heavy")
        gated: set[str] = set()
        for i in heavy_at:
            # A `case` arm is `pattern...)` and its body follows, so the
            # patterns are on the line before the `heavy=1` body, not on it.
            if i == 0:
                continue
            for token in lines[i - 1].split("|"):
                name = token.strip().rstrip(")").strip()
                if name:
                    gated.add(name)
        missing = sorted(set(MAKE_KINDS) - gated)
        self.assertEqual(
            [],
            missing,
            f"in MAKE_KINDS but not gated by the shim: {missing}. "
            "Such a target is never cached, never locked and never published.",
        )

    def test_scope_respects_explicit_package_and_coverage(self):
        argv = ["cargo", "test", "-p", "c"]
        self.assertEqual((argv, None), scope_command("test", argv, "/repo", ("crates/b/src/lib.rs",)))
        cov = ["cargo", "llvm-cov", "nextest", "--workspace"]
        self.assertEqual((cov, None), scope_command("coverage", cov, "/repo", ("crates/b/src/lib.rs",)))

    def test_scope_make_targets_through_packages(self):
        with mock.patch("gate.workspace_members", return_value=members):
            scoped, scope = scope_command("check", ["make", "check"], "/repo", ("crates/b/src/lib.rs",))
        self.assertEqual(["make", "check", "PACKAGES=-p a -p b"], scoped)
        self.assertEqual(["a", "b"], scope)
        cov = ["make", "coverage"]
        self.assertEqual((cov, None), scope_command("coverage", cov, "/repo", ("crates/b/src/lib.rs",)))
        self.assertEqual((cov, None), scope_command("coverage", cov, "/repo", ("Cargo.toml",)))
        explicit = ["make", "test", "PACKAGES=-p c"]
        self.assertEqual((explicit, None), scope_command("test", explicit, "/repo", ("crates/b/src/lib.rs",)))
        with mock.patch.dict(os.environ, {"PACKAGES": "-p c"}):
            self.assertEqual(
                (["make", "test"], None),
                scope_command("test", ["make", "test"], "/repo", ("crates/b/src/lib.rs",)),
            )

    def test_command_kind(self):
        self.assertEqual("test", command_kind(["cargo", "nextest", "run"]))
        self.assertEqual("coverage", command_kind(["cargo", "llvm-cov", "nextest"]))
        self.assertEqual("check", command_kind(["make", "-C", "x", "check"]))
        self.assertEqual("coverage", command_kind(["make", "PROFILE=", "coverage"]))
        self.assertEqual("coverage-clean", command_kind(["make", "coverage-clean"]))

    def test_purge_removes_only_coverage_entries(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": tmp}):
                repo = cache_root() / repo_key("/repo")
                for name, kind in (("a", "coverage"), ("b", "test")):
                    entry = repo / name
                    entry.mkdir(parents=True)
                    (entry / "meta.json").write_text(json.dumps({"kind": kind}))
                with contextlib.redirect_stdout(io.StringIO()):
                    _purge_coverage_cache("/repo")
                self.assertFalse((repo / "a").exists())
                self.assertTrue((repo / "b" / "meta.json").is_file())
