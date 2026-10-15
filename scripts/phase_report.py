#!/usr/bin/env python3
"""Aggregation for the phase timer's report, and the model's public import path.

`Report` turns observed lines, gaps, phases, and test results into the totals the rest of the tool reads.
The dataclasses it builds on live in `phase_model` and the printing in `phase_render`; both are re-exported here, so `from phase_report import Report, human, TestResult` keeps working.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

from phase_model import COUNTER_FIELDS, Gap, Line, Phase, ResourceReport, Sample, TestResult
from phase_render import human, render

__all__ = [
    "COUNTER_FIELDS",
    "Gap",
    "Line",
    "Phase",
    "Report",
    "ResourceReport",
    "Sample",
    "TestResult",
    "human",
    "render",
]


@dataclass
class Report:
    """Every measurement, so the residual is arithmetic rather than a guess."""

    command: list[str] = field(default_factory=list)
    started: float = 0.0
    ended: float = 0.0
    exit_code: int = 0
    phases: list[Phase] = field(default_factory=list)
    gaps: list[Gap] = field(default_factory=list)
    tests: list[TestResult] = field(default_factory=list)
    lines: list[Line] = field(default_factory=list)
    lines_seen: int = 0
    nextest_seconds: float | None = None
    tests_run: int | None = None
    cargo_compile_seconds: float | None = None
    lock_wait_observed: bool = False
    spawn_seconds: float = 0.0
    exit_seconds: float = 0.0
    resources: ResourceReport = field(default_factory=ResourceReport)

    # --- accumulation -----------------------------------------------------

    def add_gap(self, name: str, seconds: float, at: float) -> None:
        if seconds > 0:
            self.gaps.append(Gap(name=name, seconds=seconds, at=at))

    def add_line(self, offset: float, stream: str, text: str, phase: str) -> None:
        """Record one output line. Nothing is sampled: every line is kept."""
        self.lines.append(Line(offset=offset, stream=stream, text=text, phase=phase))
        self.lines_seen += 1

    def stream_counts(self) -> dict[str, int]:
        counts: dict[str, int] = {}
        for line in self.lines:
            counts[line.stream] = counts.get(line.stream, 0) + 1
        return counts

    def close_phase(self, at: float) -> None:
        for phase in reversed(self.phases):
            if phase.end is None:
                phase.end = at
                return

    def open_phase(self, name: str, at: float) -> Phase:
        self.close_phase(at)
        phase = Phase(name=name, start=at)
        self.phases.append(phase)
        return phase

    # --- derived measures -------------------------------------------------

    @property
    def wall(self) -> float:
        return max(0.0, self.ended - self.started)

    @property
    def gap_total(self) -> float:
        return sum(gap.seconds for gap in self.gaps)

    def merged_gaps(self) -> list[tuple[str, float, int]]:
        """Gaps summed per name: (name, total_seconds, occurrences)."""
        totals: dict[str, list[float]] = {}
        for gap in self.gaps:
            slot = totals.setdefault(gap.name, [0.0, 0.0])
            slot[0] += gap.seconds
            slot[1] += 1
        return [(n, v[0], int(v[1])) for n, v in totals.items()]

    def attributed(self) -> float:
        """Seconds covered by a named phase, counting overlaps once."""
        spans = [(self.started, self.started + self.spawn_seconds)]
        if self.exit_seconds:
            spans.append((self.ended - self.exit_seconds, self.ended))
        spans += [
            (p.start, p.end) for p in self.phases if p.end is not None and p.end > p.start
        ]
        spans = sorted(span for span in spans if span[1] > span[0])
        if not spans:
            return 0.0
        merged: list[list[float]] = [list(spans[0])]
        for start, end in spans[1:]:
            if start <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], end)
            else:
                merged.append([start, end])
        return min(self.wall, sum(e - s for s, e in merged) + self.gap_total)

    @property
    def residual(self) -> float:
        """Wall clock no phase accounts for. Should be near zero."""
        return max(0.0, self.wall - self.attributed())

    # --- test statistics --------------------------------------------------

    def status_counts(self) -> dict[str, int]:
        counts: dict[str, int] = {}
        for test in self.tests:
            counts[test.status] = counts.get(test.status, 0) + 1
        return counts

    def bad_statuses(self) -> list[TestResult]:
        """Every result that is not a plain pass, in the order printed."""
        return [t for t in self.tests if t.status != "PASS"]

    def distinct_test_names(self) -> int:
        return len({t.name for t in self.tests})

    def test_seconds(self) -> list[float]:
        return sorted((t.seconds for t in self.tests), reverse=True)

    def percentile(self, fraction: float) -> float:
        """Nearest-rank percentile: the smallest value at or above `fraction`.

        Nearest-rank rather than an interpolated index, so the answer is always a
        duration some test actually took. That makes "p90 = 1.5s" checkable
        against the slowest-test list, instead of a number that sits between two
        tests and matches neither.
        """
        values = sorted(t.seconds for t in self.tests)
        if not values:
            return 0.0
        rank = max(1, min(len(values), math.ceil(fraction * len(values))))
        return values[rank - 1]

    def by_binary(self) -> list[dict]:
        """Per-package totals, worst first, so a slow crate is obvious."""
        buckets: dict[str, dict] = {}
        for test in self.tests:
            slot = buckets.setdefault(
                test.binary, {"binary": test.binary, "count": 0, "total_seconds": 0.0, "max_seconds": 0.0}
            )
            slot["count"] += 1
            slot["total_seconds"] += test.seconds
            slot["max_seconds"] = max(slot["max_seconds"], test.seconds)
        rows = list(buckets.values())
        for row in rows:
            row["total_seconds"] = round(row["total_seconds"], 3)
            row["max_seconds"] = round(row["max_seconds"], 3)
        rows.sort(key=lambda r: r["total_seconds"], reverse=True)
        return rows

    def coverage_gap(self) -> int | None:
        """How many tests nextest ran that this report never saw a line for.

        A positive number means the runner was not asked to print every result,
        so the per-test numbers below are a floor, not the whole run.
        """
        if self.tests_run is None:
            return None
        return max(0, self.tests_run - self.distinct_test_names())

    # --- serialisation ----------------------------------------------------

    def to_dict(self) -> dict:
        offset = self.started
        return {
            "command": self.command,
            "wall_seconds": round(self.wall, 3),
            "exit_code": self.exit_code,
            "lock_wait_observed": self.lock_wait_observed,
            "spawn_seconds": round(self.spawn_seconds, 3),
            "exit_seconds": round(self.exit_seconds, 3),
            "residual_seconds": round(self.residual, 3),
            "nextest_seconds": self.nextest_seconds,
            "cargo_compile_seconds": self.cargo_compile_seconds,
            "tests_run": self.tests_run,
            "tests_seen": len(self.tests),
            "distinct_tests_seen": self.distinct_test_names(),
            "tests_missing_from_report": self.coverage_gap(),
            "child_lines": self.lines_seen,
            "status_counts": self.status_counts(),
            "resources": {
                "interval_seconds": self.resources.interval_seconds,
                "coverage_note": self.resources.coverage_note(self.wall),
                "unavailable_reason": self.resources.unavailable_reason,
                "errors": list(self.resources.errors),
                "totals": self.resources.totals(),
                "peak_rss_kb": self.resources.peak_rss_kb(),
                "busiest_system_intervals": [
                    {"seconds": round(span, 3), "system_seconds": round(amount, 3), "processes": n}
                    for span, amount, n in self.resources.busiest("system_seconds")
                ],
                "samples": [
                    {
                        "at_offset": round(sample.at, 3),
                        "children": sample.children,
                        **{k: round(v, 3) for k, v in sample.totals.items()},
                    }
                    for sample in self.resources.samples
                ],
            },
            "test_seconds": {
                "total": round(sum(t.seconds for t in self.tests), 3),
                "max": round(max((t.seconds for t in self.tests), default=0.0), 3),
                "p50": round(self.percentile(0.50), 3),
                "p90": round(self.percentile(0.90), 3),
                "p99": round(self.percentile(0.99), 3),
            },
            "phases": [
                {
                    "name": p.name,
                    "start_offset": round(p.start - offset, 3),
                    "end_offset": round((p.end or p.start) - offset, 3),
                    "seconds": round(p.seconds, 3),
                    "lines": p.lines,
                }
                for p in self.phases
                if p.seconds > 0
            ],
            "gap_summary": [
                {"name": n, "seconds": round(s, 3), "occurrences": c}
                for n, s, c in sorted(self.merged_gaps(), key=lambda r: -r[1])
            ],
            "longest_gaps": [
                {"name": g.name, "seconds": round(g.seconds, 3), "at_offset": round(g.at - offset, 3)}
                for g in sorted(self.gaps, key=lambda g: -g.seconds)[:50]
            ],
            "by_binary": self.by_binary(),
            "stream_counts": self.stream_counts(),
            "all_lines": [
                {
                    "at_offset": round(line.offset, 3),
                    "stream": line.stream,
                    "phase": line.phase,
                    "text": line.text,
                }
                for line in self.lines
            ],
            "non_pass": [
                {"name": t.name, "status": t.status, "seconds": t.seconds, "at_offset": round(t.at - offset, 3)}
                for t in self.bad_statuses()
            ],
            "all_tests": [
                {
                    "name": t.name,
                    "binary": t.binary,
                    "status": t.status,
                    "seconds": t.seconds,
                    "at_offset": round(t.at - offset, 3),
                }
                for t in self.tests
            ],
            "all_gaps": [
                {"name": g.name, "seconds": round(g.seconds, 3), "at_offset": round(g.at - offset, 3)}
                for g in self.gaps
            ],
        }
