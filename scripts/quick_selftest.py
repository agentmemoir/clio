#!/usr/bin/env python3
"""Self-test for scripts/quick.py and the two modules it is built from.

Every case uses a fixture: a hand-written JUnit file, or a throwaway crate in a
temporary directory. Nothing here builds or runs the workspace, so it is cheap
enough to sit in `make scripts-check`.

    python3 scripts/quick_selftest.py
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import tomllib
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import quick  # noqa: E402
import quick_estimate as est  # noqa: E402
from gate_runlog import RUNS_FILE  # noqa: E402
from gate_tree import cache_root  # noqa: E402
import scoped_cli  # noqa: E402
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


class PlanTests(unittest.TestCase):
    terms = [Term("p", modules=("a",)), Term("q", modules=("b",))]

    def setUp(self) -> None:
        # The gate's run log lives under the cache; each test writes its own, so none reads this machine's.
        self.tmp = Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))
        self.root = self.tmp / "clio"
        for patch in (
            mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(self.tmp / "cache")}),
            mock.patch.object(runner, "ROOT", self.root),
        ):
            patch.start()
            self.addCleanup(patch.stop)

    def write_runs(self, runs: list[dict]) -> None:
        path = cache_root() / RUNS_FILE
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(json.dumps(run) + "\n" for run in runs))

    def run_record(self, **changes: object) -> dict:
        record = {
            "kind": "coverage-fast", "replayed": False, "argv": ["/usr/bin/make", "coverage-fast"],
            "root": str(self.root), "branch": "b", "started": 0.0, "finished": 60.0, "test_s": 5.0,
        }
        return {**record, **changes}

    def history(self, seconds: float) -> list[est.Case]:
        return [est.Case("p", "a::x", seconds, False), est.Case("q", "b::y", seconds, False)]

    def test_the_plan_names_the_budget_and_the_three_dearest_terms(self) -> None:
        terms = [Term(name, modules=("m",)) for name in "pqrs"]
        selection = quick.Selection(terms, ["note"])
        history = [est.Case(name, "m::t", 100.0 * i, False) for i, name in enumerate("pqrs", 1)]
        lines = quick.plan_lines(selection, history, [])
        self.assertTrue(any("over the 60 s budget" in line for line in lines))
        dear = [line for line in lines if line.startswith("quick:   ")]
        self.assertEqual(len(dear), 3)
        self.assertIn("package(=s)", dear[0])

    def test_a_cheap_plan_is_within_budget_and_no_history_is_unknown(self) -> None:
        selection = quick.Selection([Term("p", modules=("a",))], ["note"])
        cheap = quick.plan_lines(selection, self.history(1.0), [])
        self.assertTrue(any("within the 60 s budget" in line for line in cheap))
        unknown = quick.plan_lines(selection, [], [])
        self.assertTrue(any("unknown" in line for line in unknown))

    def test_the_closure_estimate_judges_the_done_budget(self) -> None:
        small = quick.closure_lines(self.terms, self.history(10.0))
        self.assertTrue(any("within the 180 s budget" in line for line in small))
        big = quick.closure_lines(self.terms, self.history(1000.0))
        self.assertTrue(any("leave the coverage verdict to CI" in line for line in big))
        self.assertTrue(any("unknown" in line for line in quick.closure_lines(self.terms[:1], [])))

    def test_the_closure_estimate_adds_the_build_to_the_test_time(self) -> None:
        # Two tests that take 150 s at THREADS workers fit 180 s alone; with the 53 s build fallback they do not.
        lines = quick.closure_lines(self.terms, self.history(75.0 * quick.THREADS))
        self.assertTrue(any("203.0 s, which is build and report 53.0 s" in line for line in lines))
        self.assertTrue(any("plus tests 150.0 s over 2 test(s)" in line for line in lines))
        self.assertTrue(any("over the 180 s budget" in line for line in lines))
        self.assertTrue(any("leave the coverage verdict to CI" in line for line in lines))

    def test_the_build_is_learned_from_the_newest_fresh_run_of_this_worktree(self) -> None:
        self.write_runs([
            self.run_record(finished=60.0, test_s=5.0),  # build 55 s: the one that counts
            self.run_record(replayed=True, started=0.0, finished=900.0),
            self.run_record(root=str(self.tmp / "other"), finished=500.0),
            self.run_record(test_s=None, finished=999.0),
            self.run_record(argv=["/usr/bin/make", "coverage-fast", "PLAN=1"], finished=888.0),
        ])
        self.assertEqual(quick.build_estimate(), (55.0, "learned from this worktree's last fresh run"))
        doc = json.loads(quick.closure_json(self.terms, self.history(10.0)))
        self.assertEqual((doc["build_seconds"], doc["test_seconds"]), (55.0, 10.0 / quick.THREADS * 2))
        self.assertAlmostEqual(doc["done_seconds"], 55.0 + 10.0 * 2 / quick.THREADS)

    def test_without_a_usable_record_the_build_is_the_53_second_fallback(self) -> None:
        self.assertEqual(quick.build_estimate()[0], 53.0)
        self.write_runs([self.run_record(root=str(self.tmp / "other")), self.run_record(replayed=True)])
        self.assertEqual(quick.build_estimate(), (53.0, "fallback constant, no fresh run here recorded a test time"))
        doc = json.loads(quick.closure_json(self.terms, self.history(10.0)))
        self.assertAlmostEqual(doc["done_seconds"], 53.0 + 10.0 * 2 / quick.THREADS)

    def test_the_budget_refuses_when_build_plus_test_is_over_180_not_3_times_the_test(self) -> None:
        # Test time 60 s at THREADS workers. Build 120 s makes 180 s (fits); build 130 s makes 190 s (over).
        # Three times the test time would be 180 s both times, so only build plus test tells them apart.
        history = [est.Case("p", "a::x", 60.0 * quick.THREADS, False)]
        self.write_runs([self.run_record(finished=130.0, test_s=10.0)])  # build 120 s
        fits = json.loads(quick.closure_json([Term("p", modules=("a",))], history))
        self.assertEqual((fits["done_seconds"], fits["within"]), (180.0, True))
        self.write_runs([self.run_record(finished=140.0, test_s=10.0)])  # build 130 s
        over = json.loads(quick.closure_json([Term("p", modules=("a",))], history))
        self.assertEqual((over["done_seconds"], over["within"]), (190.0, False))
        self.assertTrue(any("over the 180 s budget" in line for line in quick.closure_lines([Term("p", modules=("a",))], history)))

    def test_the_closure_json_says_within_over_or_unknown(self) -> None:
        def doc(terms: list[Term], history: list[est.Case]) -> dict:
            return json.loads(quick.closure_json(terms, history))

        each = (quick.DONE_BUDGET_S - quick.BUILD_FALLBACK_S) * quick.THREADS / 2  # two tests that just fit
        edge = doc(self.terms, self.history(each))
        self.assertAlmostEqual(edge["done_seconds"], 180.0)
        self.assertEqual((edge["within"], edge["tests"], edge["budget_seconds"]), (True, 2, 180.0))
        self.assertFalse(doc(self.terms, self.history(each + 1))["within"])
        unknown = doc(self.terms[:1], [])
        self.assertEqual((unknown["done_seconds"], unknown["within"]), (None, None))
        nothing_to_run = doc([], [])
        self.assertEqual((nothing_to_run["done_seconds"], nothing_to_run["within"]), (0.0, True))

    def test_the_closure_prices_only_the_selected_tests_not_a_dependents(self) -> None:
        # The changed file is in `a`; `b` depends on `a`. The run selects a's module `m` alone.
        history = [est.Case("a", "m::t", 10.0, False), est.Case("a", "other::u", 500.0, False), est.Case("b", "m::t", 500.0, False)]
        doc = json.loads(quick.closure_json([Term("a", modules=("m",))], history))
        self.assertEqual((doc["packages"], doc["tests"]), (["a"], 1))
        self.assertAlmostEqual(doc["done_seconds"], quick.BUILD_FALLBACK_S + 10.0 / quick.THREADS)

    def test_the_closure_run_prices_the_selection_coverage_fast_runs(self) -> None:
        # One changed file in `a`, which `b` depends on. Coverage-fast selects only a's module test.
        rel = "crates/a/src/x.rs"
        packages = [
            {"name": "a", "deps": [], "targets": [], "tests": []},
            {"name": "b", "deps": ["a"], "targets": [], "tests": []},
        ]
        with tempfile.TemporaryDirectory() as raw:
            history = junit(Path(raw) / "junit.xml", [("a", "m::t", 10.0, False), ("a", "other::u", 500.0, False), ("b", "m::t", 500.0, False)])
            with (
                mock.patch.object(runner, "resolve_ref", return_value="origin/master"),
                mock.patch.object(quick, "history_files", return_value=[history]),
                mock.patch.object(scoped_cli, "select", return_value=([], [rel])),
                mock.patch.object(scoped_cli, "is_present", return_value=True),
                mock.patch.object(scoped_cli, "workspace_packages", return_value=packages),
                mock.patch.object(runner, "package_owners", return_value=["a"]),
                mock.patch.object(quick, "term_for", return_value=(Term("a", modules=("m",)), "note")),
                redirect_stdout(io.StringIO()) as out,
            ):
                self.assertEqual(quick.run(quick.parse(["--base", "origin/master", "--plan", "--closure", "--json"])), runner.EXIT_OK)
        doc = json.loads(out.getvalue())
        self.assertEqual((doc["packages"], doc["tests"]), (["a"], 1))
        self.assertAlmostEqual(doc["done_seconds"], quick.BUILD_FALLBACK_S + 10.0 / quick.THREADS)

    def test_a_linked_worktree_also_reads_the_main_checkouts_times(self) -> None:
        main = Path("/main")
        own = [runner.ROOT / relative for relative in quick.HISTORY]
        with mock.patch.object(quick, "main_checkout", return_value=main):
            self.assertEqual(quick.history_files(), [*own, *(main / relative for relative in quick.HISTORY)])
        with mock.patch.object(quick, "main_checkout", return_value=None):
            self.assertEqual(quick.history_files(), own)

    def test_a_new_worktree_estimates_from_the_main_checkouts_file(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            main, fresh = Path(raw) / "main", Path(raw) / "fresh"
            junit(main / "target/nextest/ci/junit.xml", [("p", "a::x", 4.0, False)])
            with mock.patch.object(quick, "main_checkout", return_value=main), mock.patch.object(runner, "ROOT", fresh):
                history = est.newest_history(quick.history_files())
        self.assertEqual([(c.binary_id, c.seconds) for c in history], [("p", 4.0)])
        self.assertEqual(est.newest_history([fresh / relative for relative in quick.HISTORY]), [])

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
            with mock.patch.object(quick, "nextest", side_effect=lambda pkgs, e, extra=(), targets=(): calls.append((pkgs, e, tuple(extra))) or 0):
                self.assertEqual(quick.run_selection(selection, [], True), 0)
            self.assertEqual(
                calls,
                [(["p"], "(binary_id(=p) & test(=a::x))", ("--no-tests=pass",)), (["p"], "package(=p)", ())],
            )
            calls.clear()
            with mock.patch.object(quick, "nextest", side_effect=lambda pkgs, e, extra=(), targets=(): calls.append((pkgs, e, tuple(extra))) or 1):
                self.assertEqual(quick.run_selection(selection, [], True), 1)
            self.assertEqual(len(calls), 1, "a failing retry must not run the full selection")

    def test_without_a_retry_the_full_selection_runs_once(self) -> None:
        calls: list[tuple] = []
        selection = quick.Selection([Term("q", modules=("m",)), Term("p")], [])
        with mock.patch.object(quick, "nextest", side_effect=lambda pkgs, e, extra=(), targets=(): calls.append((pkgs, e, tuple(extra))) or 0):
            quick.run_selection(selection, [], False)
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
        for flags in (["--json"], ["--closure"], ["--json", "--closure"]):
            with self.assertRaises(SystemExit), mock.patch.object(sys, "stderr"):
                quick.parse(["--base", "b", *flags])
        args = quick.parse(["--base", "b", "--plan", "--json", "--closure"])
        self.assertTrue(args.json and args.closure)
        with self.assertRaises(SystemExit), mock.patch.object(sys, "stderr"):
            quick.parse([])


class TargetTests(unittest.TestCase):
    def test_a_selection_of_modules_builds_only_the_lib_and_bin_targets(self) -> None:
        self.assertEqual(quick.target_flags([Term("p", modules=("a",))], []), ["--lib", "--bins"])
        self.assertEqual(quick.target_flags([Term("p", kinds=("lib", "bin"))], []), ["--lib", "--bins"])

    def test_an_integration_term_names_its_own_target_once(self) -> None:
        terms = [Term("p", binary="it"), Term("q", binary="it"), Term("r", binary="aa")]
        self.assertEqual(quick.target_flags(terms, []), ["--test", "aa", "--test", "it"])
        mixed = [Term("p", modules=("a",)), Term("q", binary="it")]
        self.assertEqual(quick.target_flags(mixed, []), ["--lib", "--bins", "--test", "it"])

    def test_a_whole_package_term_widens_only_its_own_package(self) -> None:
        packages = [{"name": "p", "tests": ["p_slow"]}, {"name": "q", "tests": ["q_b", "q_a"]}]
        mixed = [Term("p", modules=("a",)), Term("q")]
        self.assertEqual(
            quick.target_flags(mixed, packages), ["--lib", "--bins", "--test", "q_a", "--test", "q_b"]
        )
        self.assertEqual(quick.target_flags([Term("q")], packages), [])

    def test_a_retry_names_the_target_each_failure_came_from(self) -> None:
        failed = [("p", "m::a"), ("p", "m::b"), ("p::bin/p", "x"), ("p::it", "y")]
        self.assertEqual(
            quick.failed_target_flags(failed), ["--bin", "p", "--lib", "--test", "it"]
        )

    def test_the_plan_and_the_command_carry_the_target_filters(self) -> None:
        selection = quick.Selection([Term("p", modules=("a",))], ["note"])
        self.assertIn("quick: cargo targets: --lib --bins", quick.plan_lines(selection, [], []))
        with mock.patch.object(quick.subprocess, "run") as run:
            run.return_value.returncode = 0
            quick.nextest(["p"], "package(=p)", targets=["--lib", "--bins"])
        self.assertEqual(
            run.call_args.args[0][-6:], ["-p", "p", "--lib", "--bins", "-E", "package(=p)"]
        )


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
            quick.untested, "classify", return_value=quick.untested.Untested([], [], [])
        ), mock.patch.object(quick, "schema_check", return_value=1
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
