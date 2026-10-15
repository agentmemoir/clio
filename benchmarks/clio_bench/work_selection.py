"""Choosing which probes a run executes.

`select_work` groups probes by instance and, when a cap is set, samples deterministically across categories so the run is reproducible. The ranking is seeded, so the same seed selects the same probes.
"""

from __future__ import annotations

import hashlib

from .types import Instance, Probe


def _rank(seed: int, probe_id: str) -> str:
    return hashlib.sha256(f"{seed}:{probe_id}".encode("utf-8")).hexdigest()


def select_work(
    instances: list[Instance], *, max_probes: int | None, seed: int
) -> list[tuple[Instance, list[Probe]]]:
    """Choose which probes to run, grouped by instance, preserving order.

    When `max_probes` caps the run, probes are sampled round-robin across
    categories so every category is represented and the choice is seeded and
    therefore reproducible.
    """
    pairs = [(inst, probe) for inst in instances for probe in inst.probes]
    if max_probes is not None and len(pairs) > max_probes:
        by_category: dict[str, list[tuple[Instance, Probe]]] = {}
        for inst, probe in pairs:
            by_category.setdefault(probe.category, []).append((inst, probe))
        for category in by_category:
            by_category[category].sort(key=lambda pair: _rank(seed, pair[1].probe_id))
        categories = sorted(by_category)
        selected: list[tuple[Instance, Probe]] = []
        index = 0
        while len(selected) < max_probes:
            added = False
            for category in categories:
                bucket = by_category[category]
                if index < len(bucket):
                    selected.append(bucket[index])
                    added = True
                    if len(selected) >= max_probes:
                        break
            if not added:
                break
            index += 1
        pairs = selected
    grouped: dict[str, tuple[Instance, list[Probe]]] = {}
    order: list[str] = []
    for inst, probe in pairs:
        if inst.instance_id not in grouped:
            grouped[inst.instance_id] = (inst, [])
            order.append(inst.instance_id)
        grouped[inst.instance_id][1].append(probe)
    return [grouped[key] for key in order]
