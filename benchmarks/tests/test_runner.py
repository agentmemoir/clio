"""Unit and end-to-end tests for the clio-bench runner (hermetic)."""

from __future__ import annotations

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from benchmarks.clio_bench.adapters.memory import MemoryAdapter
from benchmarks.clio_bench.cli import main
from benchmarks.clio_bench.doubles import StubGenerator, StubJudge
from benchmarks.clio_bench.fetch import PinError, load_pins
from benchmarks.clio_bench.fingerprint import (
    Fingerprint,
    FingerprintError,
    unreadable_setting,
)
from benchmarks.clio_bench.report import (
    build_report,
    render_markdown,
    render_settings_lines,
)
from benchmarks.clio_bench.runner import (
    _DEFAULT_PINS,
    load_suite,
    run_suite,
    select_work,
)
from benchmarks.clio_bench.stubs import StubSuiteError
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.tests.support import synthetic_suite

_FRACTIONS = (0.6, 0.2, 0.2)


class SelectWorkTests(unittest.TestCase):
    def test_no_cap_keeps_every_probe(self) -> None:
        work = select_work(synthetic_suite().instances, max_probes=None, seed=1)
        self.assertEqual(sum(len(probes) for _, probes in work), 10)

    def test_cap_samples_across_categories(self) -> None:
        work = select_work(synthetic_suite().instances, max_probes=3, seed=1)
        selected = [probe for _, probes in work for probe in probes]
        self.assertEqual(len(selected), 3)
        self.assertGreaterEqual(len({probe.category for probe in selected}), 2)

    def test_cap_is_deterministic(self) -> None:
        first = select_work(synthetic_suite().instances, max_probes=4, seed=9)
        second = select_work(synthetic_suite().instances, max_probes=4, seed=9)
        self.assertEqual(
            [p.probe_id for _, probes in first for p in probes],
            [p.probe_id for _, probes in second for p in probes],
        )


class RunSuiteTests(unittest.TestCase):
    def _run(self, partition: str = "train"):
        return run_suite(
            "locomo",
            synthetic_suite(),
            adapter=MemoryAdapter(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition=partition,
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
        )

    def test_train_partition_runs_all_instances(self) -> None:
        results = self._run("train")
        self.assertEqual(len(results), 6)  # 3 train instances x 2 probes
        self.assertEqual({r.category for r in results}, {"single-hop", "temporal-reasoning"})

    def test_canary_partition_is_smaller(self) -> None:
        results = self._run("canary")
        self.assertEqual(len(results), 2)  # 1 canary instance x 2 probes

    def test_max_instances_limits_work(self) -> None:
        results = run_suite(
            "locomo",
            synthetic_suite(),
            adapter=MemoryAdapter(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition="train",
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
            max_instances=1,
        )
        self.assertEqual(len(results), 2)

    def test_unknown_partition_raises(self) -> None:
        with self.assertRaises(ValueError):
            self._run("nope")

    def test_retrieval_failure_is_recorded_not_fatal(self) -> None:
        class FailingRetrieve(MemoryAdapter):
            def retrieve(self, session_id, query, token_budget):
                raise RuntimeError("boom")

        results = run_suite(
            "locomo",
            synthetic_suite(),
            adapter=FailingRetrieve(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition="canary",
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
        )
        self.assertEqual(len(results), 2)
        self.assertTrue(all(r.judge is None for r in results))
        self.assertTrue(all("boom" in (r.error or "") for r in results))

    def test_ingest_failure_is_tolerated(self) -> None:
        class FailingIngest(MemoryAdapter):
            def ingest(self, session_id, turn):
                raise RuntimeError("ingest boom")

        stats: dict = {}
        results = run_suite(
            "locomo",
            synthetic_suite(),
            adapter=FailingIngest(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition="canary",
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
            stats=stats,
        )
        self.assertEqual(len(results), 2)
        self.assertEqual(stats["turns_rejected"], stats["turns_ingested"])
        self.assertGreater(stats["turns_rejected"], 0)


class ReportTests(unittest.TestCase):
    def _fingerprint(self) -> Fingerprint:
        return MemoryAdapter(get_tokenizer()).fingerprint()

    def _report(self, fingerprint: Fingerprint | None = None) -> dict:
        results = run_suite(
            "locomo",
            synthetic_suite(),
            adapter=MemoryAdapter(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition="train",
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
        )
        return build_report(
            pins=load_pins(_DEFAULT_PINS),
            run_meta={
                "suites": ["locomo"],
                "partition": "train",
                "seed": 1,
                "max_probes": None,
                "token_budget": 4096,
                "adapter": "memory",
                "judge": {"tier": "stub", "model": "stub-exact-match", "base_url": ""},
                "generator": {"model": "stub-context-echo", "base_url": ""},
            },
            suite_results={"locomo": results},
            tokenizer_name="cl100k_base(test)",
            fingerprint=fingerprint or self._fingerprint(),
        )

    def test_report_shape(self) -> None:
        report = self._report()
        self.assertEqual(report["overall"]["scored_probes"], 6)
        self.assertIn("locomo", report["suites"])
        self.assertIn("locomo", report["pins"])
        self.assertEqual(len(report["suites"]["locomo"]["probes"]), 6)

    def test_report_carries_the_system_fingerprint(self) -> None:
        report = self._report()
        block = report["systems"]["memory"]
        self.assertEqual(block["system"], "memory")
        self.assertIn("version", block)
        self.assertTrue(block["settings"])

    def test_report_labels_every_aggregate_with_tier_and_quotability(self) -> None:
        report = self._report()
        self.assertEqual(report["overall"]["tier"], "stub")
        self.assertFalse(report["overall"]["quotable"])
        self.assertFalse(report["suites"]["locomo"]["aggregate"]["quotable"])

    def test_a_report_cannot_be_built_with_an_incomplete_fingerprint(self) -> None:
        incomplete = Fingerprint(
            system="clio",
            version="1.0",
            settings={"retention_profile": unreadable_setting(tried=("no surface",))},
            required=("retention_profile",),
        )
        with self.assertRaises(FingerprintError):
            self._report(fingerprint=incomplete)

    def test_markdown_renders(self) -> None:
        markdown = render_markdown(self._report())
        self.assertIn("# clio-bench scorecard", markdown)
        self.assertIn("single-hop", markdown)

    def test_markdown_names_tier_and_probe_count_on_the_number_line(self) -> None:
        markdown = render_markdown(self._report())
        self.assertIn("[tier stub, 6 probes; not quotable]", markdown)
        # The per-category table names the run on its header row as well, so
        # no number sits under an unlabelled column block.
        self.assertIn("| Category [tier stub, 6 probes; not quotable] |", markdown)

    def test_markdown_carries_the_configuration_block(self) -> None:
        markdown = render_markdown(self._report())
        self.assertIn("## Configuration", markdown)
        self.assertIn("### memory", markdown)
        self.assertIn("configurable_settings", markdown)

    def test_settings_lines_carry_a_bounded_value(self) -> None:
        settings = {
            "retention_profile": {
                "status": "read",
                "value": {"source": "builtin", "verbosity": "selective"},
                "surface": "clio retention get --bank clio-bench-fingerprint",
            }
        }
        line = render_settings_lines(settings)[0]
        self.assertIn('"source": "builtin"', line)
        self.assertIn("read through", line)

    def test_a_long_setting_value_is_truncated_to_one_line(self) -> None:
        settings = {
            "ranking_environment": {
                "status": "read",
                "value": {"payload": "x" * 500},
                "surface": "clio ranking get --output json",
            }
        }
        line = render_settings_lines(settings)[0]
        self.assertLessEqual(len(line), 220)
        self.assertIn("…", line)


class CliTests(unittest.TestCase):
    def _run_main(self, argv: list[str]) -> int:
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
            io.StringIO()
        ):
            return main(argv)

    def test_list_stubs_returns_zero(self) -> None:
        self.assertEqual(self._run_main(["--list-stubs"]), 0)

    def test_stub_suite_is_refused(self) -> None:
        self.assertEqual(self._run_main(["--suite", "beam"]), 2)

    def test_end_to_end_hermetic_run_writes_reports(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "baseline.json"
            md = Path(tmp) / "baseline.md"
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite", return_value=synthetic_suite()
            ):
                code = self._run_main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "stub",
                        "--generator",
                        "stub",
                        "--partition",
                        "train",
                        "--max-probes",
                        "3",
                        "--out",
                        str(out),
                        "--report-md",
                        str(md),
                        "--cache-dir",
                        tmp,
                    ]
                )
            self.assertEqual(code, 0)
            payload = json.loads(out.read_text())
            self.assertEqual(payload["overall"]["scored_probes"], 3)
            self.assertIn("memory", payload["systems"])
            self.assertIn("scorecard", md.read_text())
            self.assertIn("## Configuration", md.read_text())

    def test_tier2_without_a_key_records_the_blocked_gate(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "blocked.json"
            md = Path(tmp) / "blocked.md"
            with mock.patch.dict(
                os.environ, {"CLIO_BENCH_API_KEY": "", "OPENAI_API_KEY": ""}
            ):
                code = self._run_main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "tier2",
                        "--generator",
                        "stub",
                        "--out",
                        str(out),
                        "--report-md",
                        str(md),
                        "--cache-dir",
                        tmp,
                    ]
                )
            self.assertEqual(code, 2)
            payload = json.loads(out.read_text())
            self.assertEqual(payload["status"], "blocked")
            self.assertEqual(payload["blocked_gate"]["gate"], "quotable-baseline")
            self.assertEqual(payload["blocked_gate"]["requested"]["judge"], "tier2")
            self.assertFalse(payload["blocked_gate"]["present_env"]["CLIO_BENCH_API_KEY"])
            self.assertNotIn("overall", payload)
            # The blocked record carries the adapter's fingerprint too, so a
            # report without a recorded configuration does not exist.
            self.assertEqual(payload["systems"]["memory"]["fingerprint_complete"], True)
            self.assertIn("No probe was run", md.read_text())
            self.assertIn("## Configuration", md.read_text())

    def test_fingerprint_failure_produces_no_report(self) -> None:
        class BrokenFingerprint(MemoryAdapter):
            def fingerprint(self):
                raise FingerprintError("settings could not be read")

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite", return_value=synthetic_suite()
            ), mock.patch(
                "benchmarks.clio_bench.cli._build_adapter",
                return_value=BrokenFingerprint(get_tokenizer()),
            ):
                code = self._run_main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "stub",
                        "--generator",
                        "stub",
                        "--out",
                        str(out),
                        "--cache-dir",
                        tmp,
                    ]
                )
            self.assertEqual(code, 2)
            self.assertFalse(out.exists())

    def test_pin_mismatch_fails_without_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite",
                side_effect=PinError("sha256 mismatch"),
            ):
                code = self._run_main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "stub",
                        "--generator",
                        "stub",
                        "--out",
                        str(out),
                        "--cache-dir",
                        tmp,
                    ]
                )
            self.assertEqual(code, 2)
            self.assertFalse(out.exists())

    def test_unknown_suite_is_refused(self) -> None:
        with self.assertRaises(StubSuiteError):
            load_suite("nope", load_pins(_DEFAULT_PINS), Path("."))

    def test_judge_unavailable_fails_without_report(self) -> None:
        from benchmarks.clio_bench.llm import ChatClientError

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite", return_value=synthetic_suite()
            ), mock.patch(
                "benchmarks.clio_bench.judges.llm_judge.LlmJudge.score",
                side_effect=ChatClientError("judge endpoint down"),
            ):
                code = self._run_main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "local",
                        "--generator",
                        "stub",
                        "--partition",
                        "train",
                        "--max-probes",
                        "1",
                        "--out",
                        str(out),
                        "--cache-dir",
                        tmp,
                    ]
                )
            self.assertEqual(code, 2)
            self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
