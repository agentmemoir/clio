"""Run-suite behavior: partitions, failed writes, and decline counting."""

from __future__ import annotations

import unittest
from dataclasses import replace

from benchmarks.clio_bench.adapter import AdapterError
from benchmarks.clio_bench.adapters.memory import MemoryAdapter
from benchmarks.clio_bench.doubles import StubGenerator, StubJudge
from benchmarks.clio_bench.runner import EmptyStoreError, run_suite
from benchmarks.clio_bench.scoring import aggregate
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.clio_bench.types import (
    REFUSAL_ADMISSION,
    REFUSAL_DUPLICATE,
    REFUSAL_FILTERED,
)
from benchmarks.tests.runner_doubles import (
    FRACTIONS,
    DecliningAdapter,
    FailingWriteAdapter,
    FlakyWriteAdapter,
)
from benchmarks.tests.support import synthetic_suite


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
            fractions=FRACTIONS,
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
            fractions=FRACTIONS,
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
            fractions=FRACTIONS,
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
                fractions=FRACTIONS,
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
            fractions=FRACTIONS,
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
                fractions=FRACTIONS,
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
            fractions=FRACTIONS,
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
            fractions=FRACTIONS,
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


if __name__ == "__main__":
    unittest.main()
