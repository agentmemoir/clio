"""The runner's retrieval-cost tripwire.

An adapter that reports a retrieval cost which is not the cost of the context it
returned would put that number into every probe of the report, so the run stops
before a report is written rather than publishing a number the harness cannot
stand behind. These tests drive the tripwire through both entry points: the
function that runs a suite, and the command line that writes the report.
"""

from __future__ import annotations

import contextlib
import io
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

from benchmarks.clio_bench.adapters.memory import MemoryAdapter
from benchmarks.clio_bench.cli import main
from benchmarks.clio_bench.doubles import StubGenerator, StubJudge
from benchmarks.clio_bench.runner import AccountingError, run_suite
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.tests.support import synthetic_suite

_FRACTIONS = (0.6, 0.2, 0.2)


class MisreportingAdapter(MemoryAdapter):
    """A correct adapter whose reported retrieval cost is one token too high."""

    def retrieve(self, session_id, query, token_budget):
        result = super().retrieve(session_id, query, token_budget)
        return replace(result, tokens_retrieved=result.tokens_retrieved + 1)


class RunSuiteAccountingTests(unittest.TestCase):
    def _run(self):
        return run_suite(
            "locomo",
            synthetic_suite(),
            adapter=MisreportingAdapter(get_tokenizer()),
            generator=StubGenerator(),
            judge=StubJudge(),
            partition="canary",
            seed=1,
            fractions=_FRACTIONS,
            max_probes=None,
            token_budget=4096,
            tokenizer=get_tokenizer(),
        )

    def test_a_misreported_cost_aborts_the_run(self) -> None:
        with self.assertRaises(AccountingError):
            self._run()

    def test_the_message_names_the_adapter_the_probe_and_both_numbers(self) -> None:
        with self.assertRaises(AccountingError) as caught:
            self._run()
        message = str(caught.exception)
        self.assertIn("MisreportingAdapter", message)
        self.assertIn("locomo", message)
        self.assertIn("tokens_retrieved=", message)


class CliAccountingTests(unittest.TestCase):
    def _run_main(self, argv: list[str]) -> int:
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
            io.StringIO()
        ):
            return main(argv)

    def test_a_misreported_cost_produces_no_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite", return_value=synthetic_suite()
            ), mock.patch(
                "benchmarks.clio_bench.cli._build_adapter",
                return_value=MisreportingAdapter(get_tokenizer()),
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


if __name__ == "__main__":
    unittest.main()
