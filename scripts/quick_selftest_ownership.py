#!/usr/bin/env python3
"""Self-test for the ownership and selection half of scripts/quick.py.

Split out of quick_selftest.py to keep each file under the 450-line limit. Every
case uses a fixture: a fake lookup, or a throwaway crate in a temporary directory.

    python3 scripts/quick_selftest_ownership.py
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import integration_targets  # noqa: E402
import owned_accept  # noqa: E402
import owned_scanner  # noqa: E402
import owned_tests as owned  # noqa: E402
import quick  # noqa: E402
import scoped_runner as runner  # noqa: E402
from quick_terms import Term  # noqa: E402


def fake_lookup(table: dict[str, owned.OwnedSelection]):
    def lookup(rel: str) -> owned.OwnedSelection:
        if rel not in table:
            raise owned.ResolutionError(f"{rel} is not reachable from any crate root")
        return table[rel]

    return lookup


class SelectionTests(unittest.TestCase):
    def test_each_file_selects_only_the_test_modules_it_declares(self) -> None:
        lookup = fake_lookup(
            {
                "crates/c/src/lib.rs": owned.OwnedSelection("crates/c/src/lib.rs", None, ["tests"]),
                "crates/c/src/p.rs": owned.OwnedSelection("crates/c/src/p.rs", None, ["p::tests"]),
                "crates/c/src/q.rs": owned.OwnedSelection("crates/c/src/q.rs", None, []),
                "crates/c/tests/t.rs": owned.OwnedSelection("crates/c/tests/t.rs", "t", [""]),
            }
        )
        cases = {
            "crates/c/src/lib.rs": Term("c", modules=("tests",)),
            "crates/c/src/p.rs": Term("c", modules=("p::tests",)),
            "crates/c/tests/t.rs": Term("c", binary="t"),
        }
        for rel, expected in cases.items():
            self.assertEqual(quick.term_for(rel, "c", lookup)[0], expected, rel)
        for rel in ("crates/c/src/q.rs", "crates/c/src/new.rs"):
            term, note = quick.term_for(rel, "c", lookup)
            self.assertIsNone(term, rel)
            self.assertTrue(note.startswith(f"no test selected: {rel} owns no test module"), note)
        helper = "crates/c/tests/common/mod.rs"
        term, note = quick.term_for(helper, "c", fake_lookup({helper: owned.OwnedSelection(helper, None, [])}))
        self.assertIsNone(term)
        self.assertEqual(note, f"no test selected: {helper} is a test file with no test of its own")

    def test_a_file_that_owns_no_test_module_never_widens_the_selection(self) -> None:
        packages = [{"name": "c", "dir": "crates/c", "deps": set(), "targets": []}]
        lookup = fake_lookup(
            {
                "crates/c/src/p.rs": owned.OwnedSelection("crates/c/src/p.rs", None, ["p::tests"]),
                "crates/c/src/q.rs": owned.OwnedSelection("crates/c/src/q.rs", None, []),
            }
        )
        found = quick.select(["crates/c/src/p.rs", "crates/c/src/q.rs"], packages, lookup)
        self.assertEqual(found.terms, [Term("c", modules=("p::tests",))])
        self.assertIn("no test selected: crates/c/src/q.rs owns no test module", found.notes)
        self.assertEqual(quick.select(["crates/c/src/q.rs"], packages, lookup).terms, [])

    def test_merging_unites_modules_and_a_wide_term_absorbs_the_rest(self) -> None:
        merged = quick.merge([Term("p", modules=("a",)), Term("p", modules=("b",)), Term("q")])
        self.assertEqual(merged, [Term("p", modules=("a", "b")), Term("q")])
        absorbed = quick.merge([Term("p", modules=("a",)), Term("p", reason="why"), Term("p", binary="t")])
        self.assertEqual(absorbed, [Term("p", reason="why")])

    def test_a_file_no_package_owns_fails_closed(self) -> None:
        packages = [{"name": "c", "dir": "crates/c", "deps": set(), "targets": []}]
        with self.assertRaises(runner.RunError):
            quick.select(["elsewhere/x.rs"], packages, fake_lookup({}))


class AcceptanceScopeTests(unittest.TestCase):
    def test_only_release_note_paths_skip_the_runner_lookup(self) -> None:
        self.assertFalse(owned_accept._needs_runner(("CHANGELOG.md",)))
        self.assertFalse(owned_accept._needs_runner(("changelog.d/issue.md",)))
        self.assertFalse(
            owned_accept._needs_runner(("CHANGELOG.md", "changelog.d/issue.md"))
        )

    def test_rust_config_script_and_workflow_changes_run_the_lookup(self) -> None:
        for path in (
            "crates/clio/src/lib.rs",
            "Cargo.toml",
            ".config/nextest.toml",
            "scripts/owned_tests.py",
            ".github/workflows/ci.yml",
        ):
            with self.subTest(path=path):
                self.assertTrue(owned_accept._needs_runner((path,)))

    def test_mixed_unknown_and_empty_diffs_run_the_lookup(self) -> None:
        self.assertTrue(
            owned_accept._needs_runner(("CHANGELOG.md", "crates/clio/src/lib.rs"))
        )
        self.assertTrue(owned_accept._needs_runner(None))
        self.assertTrue(owned_accept._needs_runner(()))

    def test_missing_ci_event_keeps_the_full_lookup(self) -> None:
        with mock.patch.dict(owned_accept.os.environ, {}, clear=True):
            paths = owned_accept._ci_changed_paths()

        self.assertIsNone(paths)
        self.assertTrue(owned_accept._needs_runner(paths))

    def test_ci_diff_disables_rename_detection(self) -> None:
        base_sha = "a" * 40
        head_sha = "b" * 40
        with tempfile.TemporaryDirectory() as raw:
            event_path = Path(raw) / "event.json"
            event_path.write_text(
                f'{{"pull_request":{{"base":{{"sha":"{base_sha}"}}}}}}',
                encoding="utf-8",
            )
            with mock.patch.dict(
                owned_accept.os.environ,
                {
                    "GITHUB_EVENT_PATH": str(event_path),
                    "GITHUB_EVENT_NAME": "pull_request",
                    "GITHUB_SHA": head_sha,
                },
                clear=True,
            ), mock.patch.object(
                owned_accept.subprocess,
                "run",
                return_value=mock.Mock(
                    returncode=0,
                    stdout=b"crates/clio/src/lib.rs\0changelog.d/issue.md\0",
                ),
            ) as run:
                paths = owned_accept._ci_changed_paths()

        self.assertEqual(
            paths, ("crates/clio/src/lib.rs", "changelog.d/issue.md")
        )
        command = run.call_args.args[0]
        self.assertIn("--no-renames", command)
        self.assertIn(base_sha, command)
        self.assertIn(head_sha, command)

    def test_push_event_uses_the_before_sha(self) -> None:
        base_sha = "c" * 40
        head_sha = "d" * 40
        with tempfile.TemporaryDirectory() as raw:
            event_path = Path(raw) / "event.json"
            event_path.write_text(f'{{"before":"{base_sha}"}}', encoding="utf-8")
            with mock.patch.dict(
                owned_accept.os.environ,
                {
                    "GITHUB_EVENT_PATH": str(event_path),
                    "GITHUB_EVENT_NAME": "push",
                    "GITHUB_SHA": head_sha,
                },
                clear=True,
            ), mock.patch.object(
                owned_accept.subprocess,
                "run",
                return_value=mock.Mock(returncode=0, stdout=b"CHANGELOG.md\0"),
            ) as run:
                paths = owned_accept._ci_changed_paths()

        self.assertEqual(paths, ("CHANGELOG.md",))
        command = run.call_args.args[0]
        self.assertIn(base_sha, command)
        self.assertIn(head_sha, command)


class PathAttributeTests(unittest.TestCase):
    """The real lookup on a throwaway crate, so a `#[path]` rename is followed."""

    def test_a_path_attribute_overrides_the_name_the_file_suggests(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw).resolve()
            src = root / "crates" / "fake" / "src"
            src.mkdir(parents=True)
            (src / "lib.rs").write_text("mod foo;\nmod bar;\n")
            (src / "foo.rs").write_text(
                'pub fn f() {}\n#[cfg(test)]\n#[path = "foo_odd_tests.rs"]\nmod odd;\n'
            )
            (src / "foo_odd_tests.rs").write_text("#[test]\nfn t() {}\n")
            (src / "bar.rs").write_text("pub fn g() {}\n")
            crates = {"fake": root / "crates" / "fake"}
            owned.walk_crate.cache_clear()
            with mock.patch.object(owned, "ROOT", root), mock.patch.object(
                integration_targets, "ROOT", root
            ), mock.patch.object(owned, "workspace_crates", lambda: crates):
                by_source, _ = quick.term_for("crates/fake/src/foo.rs", "fake")
                by_test_file, _ = quick.term_for("crates/fake/src/foo_odd_tests.rs", "fake")
                owns_nothing, _ = quick.term_for("crates/fake/src/bar.rs", "fake")
            owned.walk_crate.cache_clear()
        self.assertEqual(by_source, Term("fake", modules=("foo::odd",)))
        self.assertEqual(by_test_file, Term("fake", modules=("foo::odd",)))
        self.assertIsNone(owns_nothing)


class OwnershipTests(unittest.TestCase):
    """The real lookup on a throwaway crate: a file selects only the tests it owns."""

    def test_a_crate_root_selects_only_the_test_modules_it_gates(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw).resolve()
            src = root / "crates" / "fake" / "src"
            src.mkdir(parents=True)
            # `foo` is a plain declaration, so lib.rs must not select its test.
            (src / "lib.rs").write_text(
                "mod foo;\n#[cfg(test)]\nmod gated;\n"
                "#[cfg(test)]\nmod inline_tests {\n#[test]\nfn t() {}\n}\n"
                "mod plain_inline {\n#[test]\nfn p() {}\n}\n"
            )
            (src / "foo.rs").write_text("pub fn f() {}\n#[test]\nfn own() {}\n")
            (src / "gated.rs").write_text("#[test]\nfn g() {}\n")
            crates = {"fake": root / "crates" / "fake"}
            owned.walk_crate.cache_clear()
            with mock.patch.object(owned, "ROOT", root), mock.patch.object(
                integration_targets, "ROOT", root
            ), mock.patch.object(owned, "workspace_crates", lambda: crates):
                crate_root, _ = quick.term_for("crates/fake/src/lib.rs", "fake")
                foo, _ = quick.term_for("crates/fake/src/foo.rs", "fake")
            owned.walk_crate.cache_clear()
        self.assertEqual(crate_root, Term("fake", modules=("gated", "inline_tests")))
        self.assertEqual(foo, Term("fake", modules=("foo",)))

    def test_an_integration_target_claims_every_module_that_holds_a_test(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            target = Path(raw).resolve() / "suite"
            target.mkdir()
            (target / "main.rs").write_text("mod plain;\nmod helper;\n")
            (target / "plain.rs").write_text("#[test]\nfn t() {}\n")
            (target / "helper.rs").write_text("pub fn h() {}\n")
            modules = integration_targets.modules_in_target(target / "main.rs")
        self.assertEqual(modules, ["plain"])

    def test_a_test_gate_with_an_operating_system_is_a_test_gate(self) -> None:
        self.assertTrue(owned_scanner._is_gated('cfg(all(test, target_os = "linux"))'))
        self.assertFalse(owned_scanner._is_gated("cfg(not(test))"))
        self.assertTrue(owned_scanner._is_gated('cfg(all(feature = "x", test))'))
        self.assertFalse(owned_scanner._is_gated("cfg(all(unix, not(test)))"))
        self.assertFalse(owned_scanner._is_gated('cfg(all(feature = "test"))'))


if __name__ == "__main__":
    unittest.main()
