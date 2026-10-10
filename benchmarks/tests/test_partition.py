"""Unit tests for deterministic instance partitioning."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.partition import kfold_instances, partition_instances
from benchmarks.clio_bench.types import Instance


def _instances(n: int) -> list[Instance]:
    return [Instance(instance_id=f"i{n:03d}", sessions=[], probes=[]) for n in range(n)]


class PartitionTests(unittest.TestCase):
    def test_splits_cover_every_instance(self) -> None:
        parts = partition_instances(_instances(10), seed=7)
        ids = [inst.instance_id for part in parts.values() for inst in part]
        self.assertEqual(len(ids), 10)
        self.assertEqual(len(set(ids)), 10)

    def test_is_deterministic_for_same_seed(self) -> None:
        first = partition_instances(_instances(10), seed=42)
        second = partition_instances(_instances(10), seed=42)
        self.assertEqual(
            [i.instance_id for i in first["canary"]],
            [i.instance_id for i in second["canary"]],
        )

    def test_seed_changes_split(self) -> None:
        first = partition_instances(_instances(10), seed=1)
        second = partition_instances(_instances(10), seed=2)
        self.assertNotEqual(
            [i.instance_id for i in first["canary"]],
            [i.instance_id for i in second["canary"]],
        )

    def test_rejects_fractions_not_summing_to_one(self) -> None:
        with self.assertRaises(ValueError):
            partition_instances(_instances(5), seed=1, fractions=(0.5, 0.2, 0.2))

    def test_kfold_holds_out_each_instance_once(self) -> None:
        rotations = kfold_instances(_instances(10), folds=5, seed=3)
        self.assertEqual(len(rotations), 5)
        held_out = [inst.instance_id for rotation in rotations for inst in rotation["dev"]]
        self.assertEqual(sorted(held_out), sorted(i.instance_id for i in _instances(10)))
        for rotation in rotations:
            self.assertEqual(len(rotation["train"]), 8)
            self.assertEqual(len(rotation["dev"]), 2)

    def test_kfold_requires_two_folds(self) -> None:
        with self.assertRaises(ValueError):
            kfold_instances(_instances(4), folds=1, seed=1)


if __name__ == "__main__":
    unittest.main()
