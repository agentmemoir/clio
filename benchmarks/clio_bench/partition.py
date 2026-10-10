"""Deterministic, session-isolated data partitioning.

Implements the 60/20/20 train/canary/blind split and the optional 5-fold
cross-validation rotation. Splitting is by whole instance (a conversation for
LoCoMo, a haystack item for LongMemEval) so no turn or probe from one instance
leaks across partitions.
"""

from __future__ import annotations

import hashlib

from .types import Instance


def _rank(seed: int, instance_id: str) -> str:
    return hashlib.sha256(f"{seed}:{instance_id}".encode("utf-8")).hexdigest()


def partition_instances(
    instances: list[Instance],
    *,
    seed: int,
    fractions: tuple[float, float, float] = (0.6, 0.2, 0.2),
) -> dict[str, list[Instance]]:
    """Split instances into train/canary/blind deterministically.

    Instances are ordered by a seeded hash of their id, so the same seed and
    the same instance set always produce the same split. Each partition keeps
    at least one instance when there are enough instances to do so.
    """
    total = sum(fractions)
    if abs(total - 1.0) > 1e-9:
        raise ValueError(f"partition fractions must sum to 1.0, got {fractions}")
    ordered = sorted(instances, key=lambda inst: _rank(seed, inst.instance_id))
    n = len(ordered)
    train_n = int(round(n * fractions[0]))
    canary_n = int(round(n * fractions[1]))
    # Blind takes the remainder so no instance is dropped by rounding.
    train_n = min(train_n, n)
    canary_n = min(canary_n, n - train_n)
    return {
        "train": ordered[:train_n],
        "canary": ordered[train_n : train_n + canary_n],
        "blind": ordered[train_n + canary_n :],
    }


def kfold_instances(
    instances: list[Instance], *, folds: int, seed: int
) -> list[dict[str, list[Instance]]]:
    """Return `folds` rotation splits, each with its own holdout dev set.

    Fold i holds out the i-th slice as `dev` and uses the rest as `train`.
    Used for smaller datasets where a fixed 20% holdout is too thin.
    """
    if folds < 2:
        raise ValueError("folds must be >= 2")
    ordered = sorted(instances, key=lambda inst: _rank(seed, inst.instance_id))
    buckets: list[list[Instance]] = [[] for _ in range(folds)]
    for index, inst in enumerate(ordered):
        buckets[index % folds].append(inst)
    rotations: list[dict[str, list[Instance]]] = []
    for i in range(folds):
        dev = buckets[i]
        train = [inst for j, bucket in enumerate(buckets) if j != i for inst in bucket]
        rotations.append({"train": train, "dev": dev})
    return rotations
