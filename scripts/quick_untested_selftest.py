#!/usr/bin/env python3
"""Self-test for scripts/quick_untested.py and how scripts/quick.py uses it.

Git and the workspace are replaced by fixtures, so nothing here touches the real tree.

    python3 scripts/quick_untested_selftest.py
"""

from __future__ import annotations

import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import quick  # noqa: E402
import quick_untested as untested  # noqa: E402

PACKAGES = [{"name": "store", "dir": "crates/store"}, {"name": "app", "dir": "crates/app"}]


class OtherChangedTests(unittest.TestCase):
    def test_it_drops_rust_and_markdown_from_the_runners_changed_paths(self) -> None:
        paths = ["Cargo.lock", "Makefile", "a.rs", "new.json", "README.md", "sql/x.sql"]
        with mock.patch.object(untested.runner, "changed_paths", return_value=paths):
            self.assertEqual(untested.other_changed("b"), ["Cargo.lock", "Makefile", "new.json", "sql/x.sql"])


class ClassifyTests(unittest.TestCase):
    def classify(self, changed: list[str], deleted=(), owners=None) -> untested.Untested:
        with mock.patch.object(untested, "other_changed", return_value=changed), mock.patch.object(
            untested.runner, "embedders", return_value=set(owners or [])
        ):
            return untested.classify("b", PACKAGES, list(deleted))

    def test_a_non_rust_file_inside_a_package_selects_the_whole_package(self) -> None:
        found = self.classify(["crates/store/tests/data/case.json"])
        self.assertEqual([(t.package, t.is_wide) for t in found.terms], [("store", True)])
        self.assertEqual(found.left_out, [])

    def test_a_sql_file_selects_each_package_that_embeds_it(self) -> None:
        found = self.classify(["sql/001_core.sql"], owners=["app", "store"])
        self.assertEqual([t.package for t in found.terms], ["app", "store"])

    def test_a_lockfile_and_a_deleted_rust_file_are_listed_not_tested(self) -> None:
        found = self.classify(["Cargo.lock"], deleted=["crates/store/src/old.rs"])
        self.assertEqual(found.terms, [])
        self.assertEqual(found.left_out[0], "crates/store/src/old.rs (deleted; only CI checks what depended on it)")
        self.assertEqual(found.left_out[1], "Cargo.lock")

    def test_a_sql_file_nothing_embeds_is_listed(self) -> None:
        found = self.classify(["sql/unused.sql"])
        self.assertEqual((found.terms, found.left_out), ([], ["sql/unused.sql"]))


class LeftOutLinesTests(unittest.TestCase):
    def test_nothing_left_out_says_nothing(self) -> None:
        self.assertEqual(untested.left_out_lines([]), [])

    def test_the_list_is_cut_after_five_and_counts_the_rest(self) -> None:
        lines = untested.left_out_lines([f"f{n}" for n in range(7)])
        self.assertEqual(lines[0], "quick: no test selected for 7 changed file(s): f0, f1, f2, f3, f4, and 2 more")
        self.assertEqual(lines[1], "quick: CI decides those")


class RunTests(unittest.TestCase):
    def run_quick(self, changed: list[str], extra: untested.Untested) -> tuple[int, str]:
        out = io.StringIO()
        with mock.patch.object(quick.runner, "resolve_ref", return_value="x"), mock.patch.object(
            quick.runner, "changed_files", return_value=changed
        ), mock.patch.object(quick.runner, "is_present", return_value=True), mock.patch.object(
            quick.runner, "workspace_packages", return_value=PACKAGES
        ), mock.patch.object(quick.untested, "classify", return_value=extra), mock.patch.object(
            quick, "newest_history", return_value=[]
        ), mock.patch.object(quick, "run_selection", return_value=0) as run_tests, redirect_stdout(out):
            code = quick.run(quick.parse(["--base", "b", "--plan"]))
        self.assertEqual(run_tests.call_count, 0)
        return code, out.getvalue()

    def test_a_lockfile_alone_is_reported_as_untested_not_as_no_change(self) -> None:
        code, text = self.run_quick([], untested.Untested([], [], ["Cargo.lock"]))
        self.assertEqual(code, 0)
        self.assertIn("quick: no test selected for 1 changed file(s): Cargo.lock", text)
        self.assertNotIn(quick.NO_CHANGE, text)

    def test_a_clean_tree_still_says_no_changed_rust_file(self) -> None:
        code, text = self.run_quick([], untested.Untested([], [], []))
        self.assertEqual((code, text.strip()), (0, quick.NO_CHANGE))

    def test_a_package_file_alone_selects_its_package_and_the_plan_lists_the_rest(self) -> None:
        extra = untested.Untested([quick.Term("store", reason="x")], ["f -> store: whole package"], ["Cargo.lock"])
        code, text = self.run_quick([], extra)
        self.assertEqual(code, 0)
        self.assertIn("quick: f -> store: whole package", text)
        self.assertIn("quick: filter: package(=store)", text)
        self.assertIn("Cargo.lock", text)


if __name__ == "__main__":
    unittest.main()
