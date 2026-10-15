"""The clio-bench command line: reports on success and no report on failure."""

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
from benchmarks.clio_bench.fetch import PinError, load_pins
from benchmarks.clio_bench.fingerprint import FingerprintError
from benchmarks.clio_bench.runner import _DEFAULT_PINS, load_suite
from benchmarks.clio_bench.stubs import StubSuiteError
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.clio_bench.types import REFUSAL_ADMISSION
from benchmarks.tests.runner_doubles import DecliningAdapter, FailingWriteAdapter
from benchmarks.tests.support import synthetic_suite


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
