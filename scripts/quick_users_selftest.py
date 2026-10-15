#!/usr/bin/env python3
"""Self-test for scripts/owned_users.py through `quick.select`, on a throwaway crate.

A changed file that owns no test module selects the tests that use it, and widens to its
whole package only when no test uses it.

    python3 scripts/quick_users_selftest.py
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import integration_targets  # noqa: E402
import owned_tests as owned  # noqa: E402
import quick  # noqa: E402
from quick_terms import Term  # noqa: E402

CRATE = {
    "src/lib.rs": (
        "mod plan;\nmod user;\npub mod api;\nmod unused;\nmod shown;\npub use shown::S;\n"
        '#[cfg(test)]\n#[path = "helper_fixtures.rs"]\nmod fixtures;\n#[cfg(test)]\nmod other_tests;\n'
    ),
    "src/plan.rs": "mod slots;\npub fn p() { slots::s() }\n",
    "src/plan/slots.rs": "pub fn s() {}\n",
    "src/user.rs": 'pub fn u() { crate::plan::p() }\n#[cfg(test)]\n#[path = "user_tests.rs"]\nmod tests;\n',
    "src/user_tests.rs": "use crate::fixtures::f;\n#[test]\nfn t() { f() }\n",
    "src/helper_fixtures.rs": "pub fn f() {}\n",
    "src/other_tests.rs": "// plan, api, unused and fixtures are named only in this comment.\n#[test]\nfn o() {}\n",
    "src/api.rs": "pub fn a() {}\n",
    "src/unused.rs": "pub fn z() {}\n",
    "src/shown.rs": "pub struct S;\n",
    "tests/common/mod.rs": "pub fn setup() {}\n",
    "tests/a.rs": "mod common;\n#[test]\nfn a() { common::setup() }\n",
    "tests/b.rs": "#[test]\nfn b() { fake::api::a() }\n",
    "tests/d/main.rs": '#[path = "../common/mod.rs"]\nmod common;\n#[test]\nfn d() { common::setup() }\n',
    "tests/lonely/mod.rs": "pub fn l() {}\n",
}
PACKAGES = [{"name": "fake", "dir": "crates/fake", "deps": set(), "targets": [], "tests": ["a", "b", "d"]}]


class UsersTests(unittest.TestCase):
    def setUp(self) -> None:
        raw = tempfile.TemporaryDirectory()
        self.addCleanup(raw.cleanup)
        root = Path(raw.name).resolve()
        crate = root / "crates" / "fake"
        for rel, text in CRATE.items():
            (crate / rel).parent.mkdir(parents=True, exist_ok=True)
            (crate / rel).write_text(text)
        for patch in (
            mock.patch.object(owned, "ROOT", root),
            mock.patch.object(integration_targets, "ROOT", root),
            mock.patch.object(owned, "workspace_crates", lambda: {"fake": crate}),
        ):
            patch.start()
            self.addCleanup(patch.stop)
        owned.walk_crate.cache_clear()
        self.addCleanup(owned.walk_crate.cache_clear)

    def select(self, rel: str) -> tuple[list[Term], str]:
        found = quick.select([f"crates/fake/{rel}"], PACKAGES)
        return found.terms, found.notes[0]

    def test_a_shared_test_helper_selects_the_targets_that_declare_it(self) -> None:
        terms, note = self.select("tests/common/mod.rs")
        self.assertEqual(terms, [Term("fake", binary="a"), Term("fake", binary="d")])
        self.assertIn("used by", note)

    def test_a_fixture_selects_the_test_modules_that_name_it(self) -> None:
        terms, _ = self.select("src/helper_fixtures.rs")
        self.assertEqual(terms, [Term("fake", modules=("user::tests",))])

    def test_a_module_with_no_tests_selects_its_users_tests_through_a_chain(self) -> None:
        for rel in ("src/plan.rs", "src/plan/slots.rs"):
            terms, _ = self.select(rel)
            self.assertEqual(terms, [Term("fake", modules=("user::tests",))], rel)

    def test_a_public_module_selects_the_integration_target_that_names_it(self) -> None:
        terms, _ = self.select("src/api.rs")
        self.assertEqual(terms, [Term("fake", binary="b")])

    def test_a_file_no_test_uses_widens_and_says_why(self) -> None:
        cases = {
            "src/unused.rs": "no test names it",
            "tests/lonely/mod.rs": "no test names it",
            "src/shown.rs": "the crate root names it",
        }
        for rel, why in cases.items():
            terms, note = self.select(rel)
            self.assertEqual(len(terms), 1, rel)
            self.assertTrue(terms[0].is_wide, rel)
            self.assertIn("whole package", note, rel)
            self.assertIn(why, note, rel)


if __name__ == "__main__":
    unittest.main()
