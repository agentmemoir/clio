"""Scorecard assembly and rendering from a runner result."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.adapters.memory import MemoryAdapter
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
from benchmarks.clio_bench.runner import _DEFAULT_PINS, run_suite
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.tests.runner_doubles import FRACTIONS
from benchmarks.tests.support import synthetic_suite


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
            fractions=FRACTIONS,
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
                "judge": {
                    "tier": "stub",
                    "model": "stub-exact-match",
                    "revision": "stub-revision-1",
                    "served_model": "stub-exact-match",
                    "base_url": "",
                },
                "generator": {"model": "stub-context-echo", "base_url": ""},
            },
            suite_results={"locomo": results},
            tokenizer_name="cl100k_base(test)",
            fingerprint=fingerprint or self._fingerprint(),
        )

    def _report_with_judge(self, judge: dict) -> dict:
        return build_report(
            pins=load_pins(_DEFAULT_PINS),
            run_meta={
                "suites": [],
                "partition": "train",
                "seed": 1,
                "max_probes": None,
                "token_budget": 4096,
                "adapter": "memory",
                "judge": judge,
                "generator": {"model": "stub-context-echo", "base_url": ""},
            },
            suite_results={},
            tokenizer_name="cl100k_base(test)",
            fingerprint=self._fingerprint(),
        )

    def test_report_shape(self) -> None:
        report = self._report()
        self.assertEqual(report["overall"]["scored_probes"], 6)
        self.assertIn("locomo", report["suites"])
        self.assertIn("locomo", report["pins"])
        self.assertEqual(len(report["suites"]["locomo"]["probes"]), 6)

    def test_report_stamps_the_judge_identity_beside_the_model(self) -> None:
        """A reader can name the artifact and the served id that made the score."""
        judge = self._report()["run"]["judge"]
        self.assertEqual(judge["revision"], "stub-revision-1")
        self.assertEqual(judge["served_model"], "stub-exact-match")
        self.assertEqual(
            list(judge)[:4], ["tier", "model", "revision", "served_model"]
        )

    def test_a_judge_block_without_a_revision_is_refused(self) -> None:
        with self.assertRaises(PinError) as caught:
            self._report_with_judge(
                {
                    "tier": "tier1",
                    "model": "qwen2.5-1.5b-instruct",
                    "served_model": "qwen2.5-1.5b-instruct",
                    "base_url": "http://127.0.0.1:8899",
                }
            )
        self.assertIn("tier1", str(caught.exception))
        self.assertIn("revision", str(caught.exception))

    def test_a_judge_block_without_a_served_model_is_refused(self) -> None:
        with self.assertRaises(PinError) as caught:
            self._report_with_judge(
                {
                    "tier": "tier1",
                    "model": "qwen2.5-1.5b-instruct",
                    "revision": "tier1-revision",
                    "base_url": "http://127.0.0.1:8899",
                }
            )
        self.assertIn("tier1", str(caught.exception))
        self.assertIn("served_model", str(caught.exception))

    def test_the_scorecard_names_the_judge_revision(self) -> None:
        markdown = render_markdown(self._report())
        self.assertIn("revision stub-revision-1", markdown)
        self.assertIn(
            "- Judge: stub-exact-match (stub, revision stub-revision-1)", markdown
        )

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


if __name__ == "__main__":
    unittest.main()
