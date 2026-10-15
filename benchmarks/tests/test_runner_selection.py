"""Probe selection: the cap, its determinism, and its category coverage."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.runner import select_work
from benchmarks.tests.support import synthetic_suite


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


if __name__ == "__main__":
    unittest.main()
