"""Score aggregation for the clio-bench runner.

Turns per-probe results into per-category and overall accuracy, token F1
cross-checks, retrieval latency percentiles, and token-efficiency means.
"""

from __future__ import annotations

import math
import re
from collections import defaultdict

from .types import ProbeResult

_ARTICLES = re.compile(r"\b(a|an|the)\b", re.IGNORECASE)
_PUNCT = re.compile(r"[^\w\s]", re.UNICODE)


def _normalize(text: str) -> list[str]:
    lowered = text.lower().replace(",", " ")
    lowered = _ARTICLES.sub(" ", lowered)
    lowered = _PUNCT.sub(" ", lowered)
    return lowered.split()


def token_f1(prediction: str, gold: str) -> float:
    """Token-overlap F1 (the deterministic cross-check, not the primary score)."""
    pred_tokens = _normalize(prediction)
    gold_tokens = _normalize(gold)
    if not pred_tokens or not gold_tokens:
        return 0.0
    pred_counts: dict[str, int] = defaultdict(int)
    gold_counts: dict[str, int] = defaultdict(int)
    for token in pred_tokens:
        pred_counts[token] += 1
    for token in gold_tokens:
        gold_counts[token] += 1
    overlap = sum(min(pred_counts[t], gold_counts[t]) for t in pred_counts)
    if overlap == 0:
        return 0.0
    precision = overlap / len(pred_tokens)
    recall = overlap / len(gold_tokens)
    return 2 * precision * recall / (precision + recall)


def _percentile(values: list[float], percentile: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, int(round(percentile * (len(ordered) - 1))))
    return ordered[index]


def _mean(values: list[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def aggregate(results: list[ProbeResult]) -> dict:
    """Aggregate probe results into per-category and overall metrics."""
    scored = [r for r in results if r.judge is not None]
    by_category: dict[str, list[ProbeResult]] = defaultdict(list)
    for result in scored:
        by_category[result.category].append(result)

    def block(items: list[ProbeResult]) -> dict:
        accuracies = [r.judge.accuracy for r in items if r.judge is not None]  # type: ignore[union-attr]
        return {
            "count": len(items),
            "accuracy": _mean(accuracies),
            "token_f1": _mean([r.token_f1 for r in items]),
            "tokens_retrieved_mean": _mean([float(r.tokens_retrieved) for r in items]),
            "retrieval_latency_ms_p50": _percentile(
                [r.retrieval_latency_ms for r in items], 0.50
            ),
            "retrieval_latency_ms_p95": _percentile(
                [r.retrieval_latency_ms for r in items], 0.95
            ),
            "generation_latency_ms_mean": _mean(
                [r.generation_latency_ms for r in items]
            ),
        }

    return {
        "scored_probes": len(scored),
        "failed_probes": len(results) - len(scored),
        "overall": block(scored),
        "per_category": {name: block(items) for name, items in sorted(by_category.items())},
    }


def geometric_mean(values: list[float]) -> float:
    """Geometric mean used to cancel positional bias across swap passes."""
    if not values:
        return 0.0
    if any(v < 0 for v in values):
        raise ValueError("geometric mean is undefined for negative values")
    if any(v == 0 for v in values):
        return 0.0
    return math.exp(sum(math.log(v) for v in values) / len(values))