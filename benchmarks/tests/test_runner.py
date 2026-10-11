"""Unit and end-to-end tests for the clio-bench runner (hermetic)."""

from __future__ import annotations

import contextlib
import io
import json
import os
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

from benchmarks.clio_bench.adapter import AdapterError
from benchmarks.clio_bench.adapters.clio import ClioAdapterError
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
    EmptyStoreError,
    load_suite,
    run_suite,
    select_work,
)
from benchmarks.clio_bench.scoring import aggregate
from benchmarks.clio_bench.stubs import StubSuiteError
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.clio_bench.types import (
    REFUSAL_ADMISSION,
    REFUSAL_DUPLICATE,
    REFUSAL_FILTERED,
)
from benchmarks.tests.support import synthetic_suite

_FRACTIONS = (0.6, 0.2, 0.2)


class DecliningAdapter(MemoryAdapter):
    """An adapter whose every write is declined for one named reason."""

    def __init__(self, tokenizer, refusal: str) -> None:
        super().__init__(tokenizer)
        self._refusal = refusal

    def ingest(self, session_id, turn):
        stored = super().ingest(session_id, turn)
        return replace(
            stored,
            ok=False,
            error=f"declined as {self._refusal}",
            refusal=self._refusal,
        )


class FailingWriteAdapter(MemoryAdapter):
    """An adapter whose writes fail for a reason that is not a decline.

    This is the shape of the issue #21 reproduction: `clio remember` exits
    non-zero with a `config_corrupt` payload when the data directory is
    relative, and the harness used to record every such write as an admission
    refusal and then score the empty store.
    """

    def ingest(self, session_id, turn):
        raise ClioAdapterError(
            "clio remember failed with code 'config_corrupt': "
            "no key-file path could be resolved"
        )


class FlakyWriteAdapter(MemoryAdapter):
    """An adapter that fails one write and stores the rest."""

    def __init__(self, tokenizer) -> None:
        super().__init__(tokenizer)
        self._calls = 0

    def ingest(self, session_id, turn):
        self._calls += 1
        if self._calls == 1:
            raise ClioAdapterError("clio remember failed with code 'internal': boom")
        return super().ingest(session_id, turn)



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

    def test_a_broken_write_path_aborts_the_run(self) -> None:
        """Issue #21: every write fails, so nothing may be scored.

        A bank that took nothing because its writes failed is a broken run, not
        a filtered one. The run stops and names the first failure.
        """
        with self.assertRaises(EmptyStoreError) as caught:
            run_suite(
                "locomo",
                synthetic_suite(),
                adapter=FailingWriteAdapter(get_tokenizer()),
                generator=StubGenerator(),
                judge=StubJudge(),
                partition="canary",
                seed=1,
                fractions=_FRACTIONS,
                max_probes=None,
                token_budget=4096,
                tokenizer=get_tokenizer(),
                stats={},
            )
        self.assertIn("config_corrupt", str(caught.exception))

    def test_one_failed_write_costs_its_instance_but_not_the_run(self) -> None:
        """A single bad turn must not discard a whole run.

        The instance whose store is missing that turn cannot be measured, so
        its probes are recorded as failed; the other instances are scored.
        """
        stats: dict = {}
        results = run_suite(
            "locomo",
            synthetic_suite(),
            adapter=FlakyWriteAdapter(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition="train",
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
            stats=stats,
        )
        self.assertEqual(stats["turns_ingested"], 3)
        self.assertEqual(stats["turns_stored"], 2)
        self.assertEqual(stats["turns_failed"], 1)
        failed = [r for r in results if r.judge is None]
        scored = [r for r in results if r.judge is not None]
        self.assertEqual(len(failed), 2)
        self.assertEqual(len(scored), 4)
        self.assertTrue(all("could not write" in (r.error or "") for r in failed))

    def test_an_unknown_decline_kind_stops_the_run(self) -> None:
        """A decline kind the runner does not know is a bug, not a refusal.

        Absorbing it would count and label it as an admission refusal, which is
        the mislabelling issue #21 is about.
        """
        with self.assertRaises(AdapterError) as caught:
            run_suite(
                "locomo",
                synthetic_suite(),
                adapter=DecliningAdapter(get_tokenizer(), "mystery"),
                generator=StubGenerator(),
                judge=StubJudge(),
                partition="canary",
                seed=1,
                fractions=_FRACTIONS,
                max_probes=None,
                token_budget=4096,
                tokenizer=get_tokenizer(),
                stats={},
            )
        self.assertIn("mystery", str(caught.exception))

    def _declined_run(self, refusal: str) -> tuple[list, dict]:
        stats: dict = {}
        results = run_suite(
            "locomo",
            synthetic_suite(),
            adapter=DecliningAdapter(get_tokenizer(), refusal),
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
        return results, stats

    def test_a_store_that_took_nothing_is_never_scored(self) -> None:
        """Issue #21's Expected behavior: no scored numbers from an empty bank."""
        results, stats = self._declined_run(REFUSAL_ADMISSION)
        aggregate_block = aggregate(results)
        self.assertEqual(aggregate_block["scored_probes"], 0)
        self.assertEqual(aggregate_block["failed_probes"], 2)
        self.assertEqual(stats["turns_stored"], 0)
        self.assertEqual(stats["turns_rejected"], 1)
        self.assertTrue(all("stored none" in (r.error or "") for r in results))

    def test_an_instance_with_no_turns_is_not_scored(self) -> None:
        """Nothing was stored, so there is nothing to answer a probe from."""
        data = synthetic_suite()
        data.instances = [replace(inst, sessions=[]) for inst in data.instances]
        results = run_suite(
            "locomo",
            data,
            adapter=MemoryAdapter(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition="canary",
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
        )
        aggregate_block = aggregate(results)
        self.assertEqual(aggregate_block["scored_probes"], 0)
        self.assertEqual(aggregate_block["failed_probes"], 2)
        self.assertTrue(all("no turns" in (r.error or "") for r in results))

    def test_an_admission_refusal_is_counted_as_rejected(self) -> None:
        _, stats = self._declined_run(REFUSAL_ADMISSION)
        self.assertEqual(stats["turns_ingested"], 1)
        self.assertEqual(stats["turns_rejected"], 1)
        self.assertEqual(stats["turns_duplicate"], 0)
        self.assertEqual(stats["turns_filtered"], 0)

    def test_a_duplicate_is_counted_apart_from_an_admission_refusal(self) -> None:
        """A duplicate is a decline, but the store already holds the text."""
        _, stats = self._declined_run(REFUSAL_DUPLICATE)
        self.assertEqual(stats["turns_ingested"], 1)
        self.assertEqual(stats["turns_rejected"], 0)
        self.assertEqual(stats["turns_duplicate"], 1)
        self.assertEqual(stats["turns_filtered"], 0)

    def test_a_filtered_turn_is_counted_apart(self) -> None:
        """A system's own write filter is neither a gate nor a duplicate."""
        _, stats = self._declined_run(REFUSAL_FILTERED)
        self.assertEqual(stats["turns_ingested"], 1)
        self.assertEqual(stats["turns_rejected"], 0)
        self.assertEqual(stats["turns_filtered"], 1)


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

    def test_the_ingestion_line_names_the_two_counts_a_run_always_has(self) -> None:
        report = self._report()
        report["suites"]["locomo"]["ingestion"] = {
            "instances": 1,
            "turns_ingested": 3,
            "turns_stored": 3,
            "turns_rejected": 0,
            "turns_duplicate": 0,
            "turns_filtered": 0,
            "turns_failed": 0,
        }
        self.assertIn(
            "Ingestion: 1 instances, 3 turns (3 stored, 0 rejected by admission)",
            render_markdown(report),
        )

    def test_the_ingestion_line_names_every_outcome_that_happened(self) -> None:
        """Each decline kind, and a failed write, are named rather than merged."""
        report = self._report()
        report["suites"]["locomo"]["ingestion"] = {
            "instances": 1,
            "turns_ingested": 6,
            "turns_stored": 2,
            "turns_rejected": 1,
            "turns_duplicate": 1,
            "turns_filtered": 1,
            "turns_failed": 1,
        }
        self.assertIn(
            "Ingestion: 1 instances, 6 turns (2 stored, 1 rejected by admission, "
            "1 already stored, 1 dropped by the system's write filter, "
            "1 the harness could not write)",
            render_markdown(report),
        )

    def test_a_suite_with_no_scored_probe_reports_no_accuracy(self) -> None:
        """A zero would read as a measured result for a bank that held nothing."""
        report = self._report()
        report["suites"]["locomo"]["aggregate"]["scored_probes"] = 0
        markdown = render_markdown(report)
        self.assertIn("no probe was scored", markdown)
        self.assertNotIn("overall accuracy", markdown)

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

    def test_a_write_failure_produces_no_report(self) -> None:
        """The issue #21 reproduction: a failed store must not be scored.

        Every write fails with a configuration error, so a report would carry
        numbers measured against an empty store. The run exits 2 instead and
        writes nothing.
        """
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite", return_value=synthetic_suite()
            ), mock.patch(
                "benchmarks.clio_bench.cli._build_adapter",
                return_value=FailingWriteAdapter(get_tokenizer()),
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
                        "canary",
                        "--out",
                        str(out),
                        "--cache-dir",
                        tmp,
                    ]
                )
            self.assertEqual(code, 2)
            self.assertFalse(out.exists())

    def test_a_declined_store_writes_a_report_with_no_scored_probe(self) -> None:
        """A system that declines every turn is its own decision, not a fault.

        The report is written and it says that nothing was scored and why,
        rather than presenting a number measured against an empty bank.
        """
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "declined.json"
            md = Path(tmp) / "declined.md"
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite", return_value=synthetic_suite()
            ), mock.patch(
                "benchmarks.clio_bench.cli._build_adapter",
                return_value=DecliningAdapter(get_tokenizer(), REFUSAL_ADMISSION),
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
                        "canary",
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
            self.assertEqual(payload["overall"]["scored_probes"], 0)
            self.assertEqual(
                payload["suites"]["locomo"]["ingestion"]["turns_stored"], 0
            )
            scorecard = md.read_text()
            self.assertIn("no probe was scored", scorecard)
            self.assertIn("rejected by admission", scorecard)

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
