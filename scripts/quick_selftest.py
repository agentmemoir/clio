#!/usr/bin/env python3
"""Self-test for scripts/quick.py and the two modules it is built from.

Every case uses a fixture: a hand-written JUnit file, or a throwaway crate in a
temporary directory. Nothing here builds or runs the workspace, so it is cheap
enough to sit in `make scripts-check`.

    python3 scripts/quick_selftest.py
"""

from __future__ import annotations

import os
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import integration_targets  # noqa: E402
import owned_tests as owned  # noqa: E402
import quick  # noqa: E402
import quick_estimate as est  # noqa: E402
import scoped_runner as runner  # noqa: E402
from quick_terms import Term, failed_expression, union_expression  # noqa: E402

JUNIT = """<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run"><testsuite name="{suite}">{cases}</testsuite></testsuites>
"""


def junit(path: Path, cases: list[tuple[str, str, float, bool]]) -> Path:
    body = "".join(
        f'<testcase name="{name}" classname="{binary}" time="{seconds}">'
        + ('<failure message="boom"/>' if failed else "")
        + "</testcase>"
        for binary, name, seconds, failed in cases
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(JUNIT.format(suite="x", cases=body))
    return path


class TermTests(unittest.TestCase):
    def test_a_module_term_anchors_each_module(self) -> None:
        one = Term("clio-types", modules=("civil::civil_tests",))
        self.assertEqual(
            one.expression(), "package(=clio-types) & test(/^civil::civil_tests(::|$)/)"
        )
        two = Term("p", modules=("a", "b::c"))
        self.assertEqual(
            two.expression(), "package(=p) & (test(/^a(::|$)/) | test(/^b::c(::|$)/))"
        )

    def test_the_other_shapes(self) -> None:
        self.assertEqual(Term("p").expression(), "package(=p)")
        self.assertEqual(
            Term("p", kinds=("lib", "bin")).expression(),
            "package(=p) & (kind(=lib) | kind(=bin))",
        )
        self.assertEqual(
            Term("p", binary="t").expression(), "package(=p) & binary(=t)"
        )

    def test_matching_follows_nextest_binary_ids(self) -> None:
        by_module = Term("p", modules=("a",))
        self.assertTrue(by_module.matches("p", "a::tests::x"))
        self.assertTrue(by_module.matches("p::bin/p", "a"))
        self.assertFalse(by_module.matches("p", "ab::x"), "a prefix of a name is not a module")
        self.assertFalse(by_module.matches("q", "a::x"), "another package")
        self.assertFalse(by_module.matches("pp", "a::x"), "a longer package name")
        kinds = Term("p", kinds=("lib", "bin"))
        self.assertTrue(kinds.matches("p", "t") and kinds.matches("p::bin/p", "t"))
        self.assertFalse(kinds.matches("p::it", "t"), "an integration target is neither")
        target = Term("p", binary="it")
        self.assertTrue(target.matches("p::it", "anything"))
        self.assertFalse(target.matches("p::other", "anything"))
        self.assertTrue(Term("p").matches("p::it", "anything"))

    def test_a_union_groups_every_term(self) -> None:
        a, b = Term("a"), Term("b", binary="t")
        self.assertEqual(union_expression([a]), "package(=a)")
        self.assertEqual(
            union_expression([a, b]), "(package(=a)) | (package(=b) & binary(=t))"
        )
        self.assertEqual(
            failed_expression([("p", "m::x"), ("p::it", "y")]),
            "(binary_id(=p) & test(=m::x)) | (binary_id(=p::it) & test(=y))",
        )


class EstimateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))

    def test_two_failures_in_two_binaries_become_one_retry_filter(self) -> None:
        path = junit(
            self.tmp / "quick.xml",
            [("p", "a::ok", 0.1, False), ("p", "a::bad", 0.2, True), ("p::it", "worse", 0.3, True)],
        )
        failed = est.failed_cases(path)
        self.assertEqual(failed, [("p", "a::bad"), ("p::it", "worse")])
        self.assertEqual(
            failed_expression(failed),
            "(binary_id(=p) & test(=a::bad)) | (binary_id(=p::it) & test(=worse))",
        )

    def test_the_newest_file_wins_per_test(self) -> None:
        old = junit(self.tmp / "old.xml", [("p", "t", 5.0, False), ("p", "only_old", 1.0, False)])
        new = junit(self.tmp / "new.xml", [("p", "t", 2.0, False)])
        os.utime(old, (1, 1))
        os.utime(new, (2, 2))
        history = {(c.binary_id, c.name): c.seconds for c in est.newest_history([old, new])}
        self.assertEqual(history, {("p", "t"): 2.0, ("p", "only_old"): 1.0})

    def test_an_estimate_sums_matches_and_is_none_without_one(self) -> None:
        path = junit(self.tmp / "h.xml", [("p", "a::x", 1.5, False), ("p", "b::y", 2.5, False)])
        history = est.newest_history([path])
        term = Term("p", modules=("a",))
        found = est.estimate(history, lambda c: term.matches(c.binary_id, c.name))
        self.assertEqual((found.seconds, found.tests), (1.5, 1))
        self.assertIsNone(est.estimate(history, lambda c: False))
        self.assertEqual(est.wall_seconds(10.0, 2), 5.0)

    def test_missing_and_broken_files_have_no_history(self) -> None:
        broken = self.tmp / "broken.xml"
        broken.write_text("<not xml")
        self.assertEqual(est.newest_history([self.tmp / "none.xml", broken]), [])
        self.assertEqual(est.failed_cases(broken), [])


def fake_lookup(table: dict[str, owned.OwnedSelection]):
    def lookup(rel: str) -> owned.OwnedSelection:
        if rel not in table:
            raise owned.ResolutionError(f"{rel} is not reachable from any crate root")
        return table[rel]

    return lookup


class SelectionTests(unittest.TestCase):
    def test_each_row_of_the_table(self) -> None:
        lookup = fake_lookup(
            {
                "crates/c/src/p.rs": owned.OwnedSelection("crates/c/src/p.rs", None, ["p::tests"]),
                "crates/c/src/q.rs": owned.OwnedSelection("crates/c/src/q.rs", None, []),
                "crates/c/tests/t.rs": owned.OwnedSelection("crates/c/tests/t.rs", "t", [""]),
            }
        )
        cases = {
            "crates/c/src/lib.rs": Term("c", kinds=("lib", "bin")),
            "crates/c/src/main.rs": Term("c", kinds=("lib", "bin")),
            "crates/c/src/p.rs": Term("c", modules=("p::tests",)),
            "crates/c/tests/t.rs": Term("c", binary="t"),
        }
        for rel, expected in cases.items():
            self.assertEqual(quick.term_for(rel, "c", lookup)[0], expected, rel)
        owns_none, note = quick.term_for("crates/c/src/q.rs", "c", lookup)
        self.assertTrue(owns_none.is_wide)
        self.assertIn("owns no test module", note)
        unmapped, note = quick.term_for("crates/c/src/new.rs", "c", lookup)
        self.assertTrue(unmapped.is_wide)
        self.assertIn("not reachable", note)

    def test_merging_unites_modules_and_a_wide_term_absorbs_the_rest(self) -> None:
        merged = quick.merge([Term("p", modules=("a",)), Term("p", modules=("b",)), Term("q")])
        self.assertEqual(merged, [Term("p", modules=("a", "b")), Term("q")])
        absorbed = quick.merge([Term("p", modules=("a",)), Term("p", reason="why"), Term("p", binary="t")])
        self.assertEqual(absorbed, [Term("p", reason="why")])

    def test_a_file_no_package_owns_fails_closed(self) -> None:
        packages = [{"name": "c", "dir": "crates/c", "deps": set(), "targets": []}]
        with self.assertRaises(runner.RunError):
            quick.select(["elsewhere/x.rs"], packages, fake_lookup({}))


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
        self.assertTrue(owns_nothing.is_wide)


class PlanTests(unittest.TestCase):
    def history(self, seconds: float) -> list[est.Case]:
        return [est.Case("p", "a::x", seconds, False), est.Case("q", "b::y", seconds, False)]

    def test_the_plan_names_the_budget_and_the_three_dearest_terms(self) -> None:
        terms = [Term(name, modules=("m",)) for name in "pqrs"]
        selection = quick.Selection(terms, ["note"])
        history = [est.Case(name, "m::t", 100.0 * i, False) for i, name in enumerate("pqrs", 1)]
        lines = quick.plan_lines(selection, history)
        self.assertTrue(any("over the 60 s budget" in line for line in lines))
        dear = [line for line in lines if line.startswith("quick:   ")]
        self.assertEqual(len(dear), 3)
        self.assertIn("package(=s)", dear[0])

    def test_a_cheap_plan_is_within_budget_and_no_history_is_unknown(self) -> None:
        selection = quick.Selection([Term("p", modules=("a",))], ["note"])
        cheap = quick.plan_lines(selection, self.history(1.0))
        self.assertTrue(any("within the 60 s budget" in line for line in cheap))
        unknown = quick.plan_lines(selection, [])
        self.assertTrue(any("unknown" in line for line in unknown))

    def test_the_closure_estimate_judges_the_done_budget(self) -> None:
        small = quick.closure_lines(["p", "q"], self.history(10.0))
        self.assertTrue(any("within the 180 s budget" in line for line in small))
        big = quick.closure_lines(["p", "q"], self.history(1000.0))
        self.assertTrue(any("leave the coverage verdict to CI" in line for line in big))
        self.assertTrue(any("likely reads low" in line for line in big))
        self.assertTrue(any("unknown" in line for line in quick.closure_lines(["p"], [])))

    def test_the_json_plan_carries_package_kind_and_pattern_per_term(self) -> None:
        import json

        selection = quick.Selection([Term("p", modules=("a",))], [])
        doc = json.loads(quick.plan_json(selection, ["f.rs"]))
        self.assertEqual(doc["files"], ["f.rs"])
        self.assertEqual(doc["terms"][0]["package"], "p")
        self.assertEqual(doc["terms"][0]["patterns"], ["^a(::|$)"])
        self.assertEqual(doc["terms"][0]["kinds"], [])


class RunTests(unittest.TestCase):
    def test_a_retry_runs_the_failures_first_and_stops_if_they_still_fail(self) -> None:
        selection = quick.Selection([Term("p")], [])
        calls: list[tuple] = []
        with mock.patch.object(quick, "failed_cases", return_value=[("p", "a::x")]):
            with mock.patch.object(quick, "nextest", side_effect=lambda pkgs, e, extra=(): calls.append((pkgs, e, tuple(extra))) or 0):
                self.assertEqual(quick.run_selection(selection, True), 0)
            self.assertEqual(
                calls,
                [(["p"], "(binary_id(=p) & test(=a::x))", ("--no-tests=pass",)), (["p"], "package(=p)", ())],
            )
            calls.clear()
            with mock.patch.object(quick, "nextest", side_effect=lambda pkgs, e, extra=(): calls.append((pkgs, e, tuple(extra))) or 1):
                self.assertEqual(quick.run_selection(selection, True), 1)
            self.assertEqual(len(calls), 1, "a failing retry must not run the full selection")

    def test_without_a_retry_the_full_selection_runs_once(self) -> None:
        calls: list[tuple] = []
        selection = quick.Selection([Term("q", modules=("m",)), Term("p")], [])
        with mock.patch.object(quick, "nextest", side_effect=lambda pkgs, e, extra=(): calls.append((pkgs, e, tuple(extra))) or 0):
            quick.run_selection(selection, False)
        self.assertEqual(calls, [(["p", "q"], quick.union_expression(selection.terms), ())])

    def test_the_command_uses_the_quick_profile_and_the_hermetic_environment(self) -> None:
        with mock.patch.object(quick.subprocess, "run") as run:
            run.return_value.returncode = 0
            quick.nextest(["a", "b"], "package(=a)")
        command = run.call_args.args[0]
        self.assertEqual(command[:4], ["cargo", "nextest", "run", "--locked"])
        self.assertIn("quick", command)
        self.assertEqual(command[-6:], ["-p", "a", "-p", "b", "-E", "package(=a)"])
        env = run.call_args.kwargs["env"]
        self.assertEqual(env["GATE_ACTIVE"], "1")
        self.assertTrue(env["CLIO_DEPLOYMENT_CONFIG"].endswith("no-deployment-overlay.json"))

    def test_options_that_need_a_plan_are_refused_without_one(self) -> None:
        for flags in (["--json"], ["--closure"], ["--plan", "--json", "--closure"]):
            with self.assertRaises(SystemExit), mock.patch.object(sys, "stderr"):
                quick.parse(["--base", "b", *flags])
        with self.assertRaises(SystemExit), mock.patch.object(sys, "stderr"):
            quick.parse([])


class SchemaCheckTests(unittest.TestCase):
    def test_it_runs_first_only_when_the_change_touches_clio_mcp(self) -> None:
        with mock.patch.object(quick.subprocess, "run") as run:
            self.assertEqual(quick.schema_check(["crates/clio-store/src/a.rs"]), 0)
            run.assert_not_called()
            run.return_value.returncode = 1
            self.assertEqual(quick.schema_check(["crates/clio-store/src/a.rs", "crates/clio-mcp/src/b.rs"]), 1)
            self.assertEqual(run.call_args.args[0][-1], str(quick.SCHEMA_CHECK))

    def test_a_failed_schema_check_stops_the_run_before_any_test(self) -> None:
        args = quick.parse(["--base", "b"])
        with mock.patch.object(quick.runner, "resolve_ref", return_value="x"), mock.patch.object(
            quick.runner, "changed_files", return_value=["crates/clio-mcp/src/b.rs"]
        ), mock.patch.object(quick.runner, "is_present", return_value=True), mock.patch.object(
            quick.runner, "workspace_packages", return_value=[]
        ), mock.patch.object(quick, "select", return_value=quick.Selection([Term("p")], [])), mock.patch.object(
            quick, "schema_check", return_value=1
        ), mock.patch.object(quick, "run_selection") as run_tests, mock.patch.object(quick, "newest_history", return_value=[]):
            with mock.patch("builtins.print"):
                self.assertEqual(quick.run(args), 1)
        run_tests.assert_not_called()


class ProfileTests(unittest.TestCase):
    def test_the_quick_profile_stops_early_never_retries_and_keeps_the_thread_cap(self) -> None:
        config = tomllib.loads((runner.ROOT / ".config" / "nextest.toml").read_text())
        profile = config["profile"]["quick"]
        self.assertTrue(profile["fail-fast"])
        self.assertEqual(profile["retries"], 0)
        self.assertEqual(str(profile["test-threads"]), runner.TEST_THREADS)
        self.assertEqual(profile["junit"]["path"], "junit.xml")
        self.assertEqual(quick.QUICK_JUNIT, runner.ROOT / "target/nextest/quick/junit.xml")


if __name__ == "__main__":
    unittest.main()
