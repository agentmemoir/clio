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
from scoped_runner import RunError  # noqa: E402

PACKAGES = [{"name": "store", "dir": "crates/store"}, {"name": "app", "dir": "crates/app"}]


def fake_git(committed: str = "", staged: str = "", unstaged: str = "", untracked: str = ""):
    def git(*args: str) -> str:
        if args[0] == "ls-files":
            return untracked
        if args[-1].endswith("..HEAD"):
            return committed
        return staged if args[-1] == "--cached" else unstaged

    return git


class OtherChangedTests(unittest.TestCase):
    def test_it_reads_all_four_queries_and_drops_rust_and_markdown(self) -> None:
        git = fake_git("Cargo.lock\na.rs\n", "sql/x.sql\nREADME.md\n", "Makefile\n", "new.json\0")
        with mock.patch.object(untested.runner, "merge_base", return_value="abc"), mock.patch.object(
            untested.runner, "git", side_effect=git
        ):
            self.assertEqual(untested.other_changed("b"), ["Cargo.lock", "Makefile", "new.json", "sql/x.sql"])


class ClassifyTests(unittest.TestCase):
    def classify(self, changed: list[str], deleted=(), owners=None) -> untested.Untested:
        with mock.patch.object(untested, "other_changed", return_value=changed), mock.patch.object(
            untested, "sql_owners", return_value=owners or []
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


class SqlOwnersTests(unittest.TestCase):
    def test_no_match_is_an_empty_answer_not_an_error(self) -> None:
        with mock.patch.object(untested.runner, "git", side_effect=RunError("exit 1")):
            self.assertEqual(untested.sql_owners("sql/x.sql", PACKAGES), [])

    def test_owners_come_from_the_files_that_name_it(self) -> None:
        with mock.patch.object(untested.runner, "git", return_value="crates/store/src/m.rs\ncrates/app/src/c.rs\n"):
            self.assertEqual(untested.sql_owners("sql/x.sql", PACKAGES), ["app", "store"])


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
