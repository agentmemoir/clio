"""Unit tests for token-F1, aggregation, and geometric mean."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.scoring import aggregate, geometric_mean, token_f1
from benchmarks.clio_bench.types import JudgeScore, ProbeResult


def _result(probe_id: str, category: str, accuracy: float, f1: float) -> ProbeResult:
    return ProbeResult(
        probe_id=probe_id,
        suite="locomo",
        instance_id="c1",
        category=category,
        question="q",
        gold_answer="g",
        candidate_answer="c",
        retrieved_context="",
        judge=JudgeScore(
            accuracy=accuracy,
            temporal_correctness=True,
            hallucination_penalty=0.0,
            explanation="x",
        ),
        token_f1=f1,
        retrieval_latency_ms=1.0,
        generation_latency_ms=2.0,
        tokens_retrieved=3,
    )


class TokenF1Tests(unittest.TestCase):
    def test_identical_is_one(self) -> None:
        self.assertEqual(token_f1("the quick brown fox", "the quick brown fox"), 1.0)

    def test_disjoint_is_zero(self) -> None:
        self.assertEqual(token_f1("alpha beta", "gamma delta"), 0.0)

    def test_partial_overlap(self) -> None:
        self.assertAlmostEqual(token_f1("red green", "red blue"), 0.5)

    def test_empty_is_zero(self) -> None:
        self.assertEqual(token_f1("", "gold"), 0.0)


class GeometricMeanTests(unittest.TestCase):
    def test_geometric_mean_cancels_position(self) -> None:
        self.assertAlmostEqual(geometric_mean([0.9, 0.4]), (0.9 * 0.4) ** 0.5)

    def test_zero_dominates(self) -> None:
        self.assertEqual(geometric_mean([1.0, 0.0]), 0.0)

    def test_empty_is_zero(self) -> None:
        self.assertEqual(geometric_mean([]), 0.0)

    def test_negative_rejected(self) -> None:
        with self.assertRaises(ValueError):
            geometric_mean([1.0, -1.0])


class AggregateTests(unittest.TestCase):
    def test_groups_by_category_and_overall(self) -> None:
        results = [
            _result("p1", "single-hop", 1.0, 1.0),
            _result("p2", "single-hop", 0.0, 0.0),
            _result("p3", "temporal-reasoning", 0.5, 0.25),
        ]
        report = aggregate(results)
        self.assertEqual(report["scored_probes"], 3)
        self.assertAlmostEqual(report["overall"]["accuracy"], 0.5)
        self.assertAlmostEqual(report["per_category"]["single-hop"]["accuracy"], 0.5)
        self.assertAlmostEqual(
            report["per_category"]["temporal-reasoning"]["accuracy"], 0.5
        )
        self.assertEqual(report["per_category"]["single-hop"]["count"], 2)

    def test_unscored_results_counted_as_failed(self) -> None:
        failed = _result("p1", "single-hop", 0.0, 0.0)
        failed.judge = None
        report = aggregate([failed])
        self.assertEqual(report["scored_probes"], 0)
        self.assertEqual(report["failed_probes"], 1)

    def test_empty_results(self) -> None:
        report = aggregate([])
        self.assertEqual(report["overall"]["accuracy"], 0.0)
        self.assertEqual(report["per_category"], {})


if __name__ == "__main__":
    unittest.main()
