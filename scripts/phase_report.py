#!/usr/bin/env python3
"""Data model, aggregation, and rendering for scripts/phase_timer.py.

Separated from the runner so the measurement code stays about measurement. Every
timing the runner observes lands here: each output line's arrival instant, each
silence between lines, each phase span, each individual test result, and each
system-counter sample taken while the child ran. None of it is sampled down or
truncated, because a dropped timing is indistinguishable from a timing that did
not happen.

Imported by phase_timer.py; not meant to be run directly.
"""

from __future__ import annotations

import math
import sys
from dataclasses import dataclass, field
from typing import TextIO

# Counters sampled from the kernel for the child and its descendants. Every
# field is a cumulative total for the process tree at one instant, so the cost of
# this work is a subtraction between two samples rather than a measurement of its
# own. On this workspace the decisive number was `sys` time exceeding `user` time
# threefold during a build, which no amount of reading the child's output can
# reveal: a linking run prints nothing while it burns kernel time.
COUNTER_FIELDS = (
    "user_seconds",
    "system_seconds",
    "max_rss_kb",
    "minor_faults",
    "major_faults",
    "voluntary_ctx_switches",
    "involuntary_ctx_switches",
    "read_bytes",
    "write_bytes",
    "peak_rss_kb",
)


@dataclass
class Sample:
    """One reading of the process tree's kernel counters.

    `at` is the wall-clock offset from the run's start, so a sample lines up with
    the phase that was open when it was taken. `children` is the total across the
    child and every descendant still alive at that instant.
    """

    at: float
    children: int
    totals: dict[str, float]

    def delta(self, later: "Sample") -> dict[str, float]:
        """Counter growth between two samples.

        Counters that can legitimately fall (a process exits, so its RSS and
        fault counts leave the tree) are floored at zero rather than allowed to
        go negative and corrupt a total.
        """
        return {
            name: max(0.0, later.totals.get(name, 0.0) - self.totals.get(name, 0.0))
            for name in COUNTER_FIELDS
        }


@dataclass
class ResourceReport:
    """Kernel-counter totals for a run, plus where they were spent.

    Held separately from `Report` because these are sampled, not observed: the
    line parser sees every line, while this sees the tree at fixed intervals. The
    distinction matters when the two disagree, and `coverage_note` says so rather
    than letting a reader assume the sampling is exhaustive.
    """

    samples: list[Sample] = field(default_factory=list)
    interval_seconds: float = 0.0
    unavailable_reason: str | None = None
    #: Exact end-of-run counters from the OS accounting API, which unlike `ps`
    #: splits user time from system time. None when unavailable.
    exact: dict[str, float] | None = None
    #: Failures the sampler hit. Reported rather than swallowed: a run whose
    #: sampler died must not look like a run that used no CPU.
    errors: list[str] = field(default_factory=list)

    def add(self, sample: Sample) -> None:
        self.samples.append(sample)

    def note_error(self, message: str) -> None:
        # Deduplicated: a sampler that fails the same way every tick would
        # otherwise push thousands of identical lines into the report.
        if message not in self.errors:
            self.errors.append(message)

    @property
    def covered_seconds(self) -> float:
        if len(self.samples) < 2:
            return 0.0
        return self.samples[-1].at - self.samples[0].at

    def total(self, name: str) -> float:
        """Growth of one counter across the run.

        The exact end-of-run figure wins when available: it covers the child's
        whole life including its final moments, and it splits user from system.
        The samples are the fallback, and the gap between the two is exactly the
        tail the sampler missed.
        """
        if self.exact is not None and name in self.exact:
            return self.exact[name]
        if len(self.samples) < 2:
            return 0.0
        return sum(
            earlier.delta(later).get(name, 0.0)
            for earlier, later in zip(self.samples, self.samples[1:])
        )

    def sampled(self, name: str) -> float:
        """Counter growth as the sampler saw it, ignoring the exact figures."""
        if len(self.samples) < 2:
            return 0.0
        return sum(
            earlier.delta(later).get(name, 0.0)
            for earlier, later in zip(self.samples, self.samples[1:])
        )

    def totals(self) -> dict[str, float]:
        return {name: round(self.total(name), 3) for name in COUNTER_FIELDS}

    def peak_rss_kb(self) -> float:
        if not self.samples:
            return 0.0
        return max(sample.totals.get("peak_rss_kb", 0.0) for sample in self.samples)

    def sampling_gap_seconds(self, name: str) -> float:
        """Counter growth the sampler missed, because it only saw the tree live.

        Positive when the exact figure is larger than the sampled one, which is
        the cost of a process tree that partly existed between two ticks. Printed
        so a reader can tell sampled data from a total.
        """
        if self.exact is None:
            return 0.0
        return max(0.0, self.exact.get(name, 0.0) - self.sampled(name))

    def busiest(self, name: str, top: int = 5) -> list[tuple[float, float, int]]:
        """Intervals that consumed the most of `name`: (seconds, amount, processes)."""
        windows: list[tuple[float, float, int]] = []
        for earlier, later in zip(self.samples, self.samples[1:]):
            amount = earlier.delta(later).get(name, 0.0)
            windows.append((later.at - earlier.at, amount, later.children))
        return sorted(windows, key=lambda w: -w[1])[:top]

    def coverage_note(self, wall_seconds: float) -> str:
        if not self.samples:
            if self.errors:
                return f"no samples taken; sampler reported {self.errors[0]}"
            return (
                "no samples taken (the run was shorter than one sampling"
                " interval, so the tree was never observed live)"
            )
        covered = self.covered_seconds
        if wall_seconds <= 0:
            return f"{len(self.samples)} samples, no wall clock to compare against"
        return (
            f"{len(self.samples)} samples every {self.interval_seconds:g}s covering"
            f" {covered:.1f}s of {wall_seconds:.1f}s wall"
            f" ({covered / wall_seconds * 100:.0f}%)"
        )


def human(seconds: float) -> str:
    """Render seconds the way a person reads a stopwatch."""
    if seconds < 1:
        return f"{seconds * 1000:.0f}ms"
    if seconds < 60:
        return f"{seconds:.1f}s"
    minutes, rest = divmod(int(seconds), 60)
    if minutes < 60:
        return f"{minutes}m{rest:02d}s"
    hours, minutes = divmod(minutes, 60)
    return f"{hours}h{minutes:02d}m{rest:02d}s"


@dataclass
class Gap:
    """Unprinted time between two output lines, named after what surrounded it.

    A runner spends most of its wall clock printing nothing: spawning a process,
    waiting for it, reaping it. Those intervals are real cost. Each one is kept
    individually so the longest single stall is visible; a merged total alone
    hides a 272s pause behind an average of 24ms.
    """

    name: str
    seconds: float
    at: float


@dataclass
class Line:
    """One line of the child's output, with the instant it arrived.

    Recorded for every line on both streams. A line is the only evidence that
    exists for where the run was at a given moment, so keeping the arrival
    offset of all of them is what lets a later reader reconstruct the timeline
    instead of trusting a summary.
    """

    offset: float
    stream: str
    text: str
    phase: str


@dataclass
class TestResult:
    """One test result line, with the instant it was printed."""

    name: str
    seconds: float
    status: str
    at: float

    @property
    def binary(self) -> str:
        """The test binary or package the result came from.

        nextest prints `<binary-id> <test-path>`; the first field is the crate
        or bin target, which is the only place a slow test can be attributed to
        a package without re-parsing names by hand.
        """
        return self.name.split(" ", 1)[0] if " " in self.name else "(unknown)"


@dataclass
class Phase:
    """A named span of the run, measured from observed lines."""

    name: str
    start: float
    end: float | None = None
    lines: int = 0

    @property
    def seconds(self) -> float:
        return 0.0 if self.end is None else self.end - self.start


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


def _render_resources(report: Report, stream: TextIO, wall: float) -> None:
    """Print what the kernel charged the run for, next to what the child printed.

    The child's own lines cannot show this. A build that spends three times more
    time in the kernel than on the CPU prints exactly the same output as one that
    does not, which is why a run can be 80% build with no line accounting for it.
    """
    resources = report.resources
    print("", file=stream)
    print("kernel counters for the child and its live descendants:", file=stream)
    exact = "exact end-of-run totals" if resources.exact is not None else "sampled totals"
    print(f"  coverage: {resources.coverage_note(wall)}; source: {exact}", file=stream)
    if resources.unavailable_reason:
        print(f"  note    : {resources.unavailable_reason}", file=stream)
    for error in resources.errors:
        print(f"  sampler : {error} (sampling continued)", file=stream)

    totals = resources.totals()
    user = totals["user_seconds"]
    system = totals["system_seconds"]
    cpu = user + system
    if cpu > 0:
        missed = resources.sampling_gap_seconds("system_seconds")
        suffix = f"  (sampler missed {human(missed)})" if missed > 0.05 else ""
        print(
            f"  cpu      : {human(user)} user + {human(system)} system"
            f" = {human(cpu)}"
            f"  ({system / cpu * 100:.0f}% in the kernel){suffix}",
            file=stream,
        )
        verdict = (
            "kernel-bound: this run waited on the kernel more than it computed."
            " Reading the child's output will not explain it."
            if system > user
            else "cpu-bound."
        )
        print(f"             {verdict}", file=stream)
    print(
        f"  memory   : peak rss"
        f" {human(resources.peak_rss_kb() / 1024)} summed over the tree",
        file=stream,
    )
    faults = totals["minor_faults"] + totals["major_faults"]
    if faults:
        print(
            f"  faults   : {faults:,.0f} minor, {totals['major_faults']:,.0f} major",
            file=stream,
        )
    io_bytes = totals["read_bytes"] + totals["write_bytes"]
    if io_bytes:
        print(
            f"  i/o      : {human(totals['read_bytes'])} read,"
            f" {human(totals['write_bytes'])} written"
            f"  ({human(io_bytes)} total)",
            file=stream,
        )
    switches = totals["voluntary_ctx_switches"] + totals["involuntary_ctx_switches"]
    if switches:
        print(
            f"  context  : {switches:,.0f} switches"
            f" ({totals['involuntary_ctx_switches']:,.0f} involuntary)",
            file=stream,
        )

    busiest = resources.busiest("system_seconds", top=5)
    if busiest and max(amount for _, amount, _ in busiest) > 0.05:
        print("", file=stream)
        print("  where the kernel time went (interval, system seconds, live processes):", file=stream)
        for span, amount, children in busiest:
            if amount <= 0.05:
                break
            print(f"    {human(span):>8}  {amount:7.2f}s  n={children}", file=stream)


def render(report: Report, stream: TextIO = sys.stderr) -> None:
    """Print the whole breakdown, with the residual and the red tests called out."""
    wall = report.wall
    share = (lambda s: (s / wall * 100) if wall else 0.0)

    print("=" * 78, file=stream)
    print("PHASE TIMING", file=stream)
    print("=" * 78, file=stream)
    print(f"command : {' '.join(report.command)}", file=stream)
    print(f"exit    : {report.exit_code}", file=stream)
    print(f"wall    : {human(wall)}", file=stream)
    print(f"attributed: {human(report.attributed())}", file=stream)
    print(f"residual : {human(report.residual)}", file=stream)
    print(
        f"  spawn {human(report.spawn_seconds)} | teardown {human(report.exit_seconds)}"
        "   (print no output line; measured anyway)",
        file=stream,
    )
    if report.residual > max(1.0, 0.02 * wall):
        print(
            "  !! residual exceeds 2% of wall clock: some time is attributed to no"
            " phase. Do not guess what it was.",
            file=stream,
        )
    if report.exit_code != 0 and not report.bad_statuses():
        print(
            f"  !! exit {report.exit_code} but no failing result line was captured;"
            " the failure detail is in the raw log.",
            file=stream,
        )

    missing = report.coverage_gap()
    if missing:
        print(
            f"  !! nextest ran {report.tests_run} tests but only"
            f" {report.distinct_test_names()} result lines were seen"
            f" ({missing} missing). Re-run with --status-level all for every timing.",
            file=stream,
        )

    print("", file=stream)
    print(
        "stage timeline, in order (every stage is measured, none is inferred):",
        file=stream,
    )
    span = report.spawn_seconds
    print(
        f"  {human(span):>10}  {share(span):5.1f}%  timer startup"
        "  (before the child exists)",
        file=stream,
    )
    for phase in report.phases:
        if phase.seconds <= 0:
            continue
        print(
            f"  {human(phase.seconds):>10}  {share(phase.seconds):5.1f}%"
            f"  {phase.name}  ({phase.lines} lines)"
            f"  [t+{human(phase.start - report.started)}"
            f" .. t+{human((phase.end or phase.start) - report.started)}]",
            file=stream,
        )
    if report.exit_seconds > 0:
        print(
            f"  {human(report.exit_seconds):>10}  {share(report.exit_seconds):5.1f}%"
            f"  teardown  (after the last line)"
            f"  [t+{human(report.ended - report.exit_seconds - report.started)}"
            f" .. t+{human(report.ended - report.started)}]",
            file=stream,
        )
    print(
        f"  {human(report.gap_total):>10}  {share(report.gap_total):5.1f}%"
        "  ^ of which silent (runner working between lines); itemised below",
        file=stream,
    )

    print("", file=stream)
    print("silent gaps (runner working between lines), merged by cause:", file=stream)
    for name, seconds, count in sorted(report.merged_gaps(), key=lambda r: -r[1]):
        print(
            f"  {human(seconds):>10}  {share(seconds):5.1f}%"
            f"  {name}  x{count}, avg {human(seconds / count)}",
            file=stream,
        )

    longest = sorted(report.gaps, key=lambda g: -g.seconds)[:10]
    if longest:
        print("", file=stream)
        print("longest single silent gaps:", file=stream)
        for gap in longest:
            print(
                f"  {human(gap.seconds):>10}  at t+{human(gap.at - report.started)}"
                f"  {gap.name}",
                file=stream,
            )

    if report.tests:
        print("", file=stream)
        print(f"test results: {len(report.tests)} lines,", file=stream)
        print(
            f"  total {human(sum(t.seconds for t in report.tests))}"
            f"  max {human(max(t.seconds for t in report.tests))}"
            f"  p50 {human(report.percentile(0.50))}"
            f"  p90 {human(report.percentile(0.90))}"
            f"  p99 {human(report.percentile(0.99))}",
            file=stream,
        )
        print(
            "  by status: "
            + ", ".join(f"{k}={v}" for k, v in sorted(report.status_counts().items())),
            file=stream,
        )

        print("", file=stream)
        print(
            "test time by binary, heaviest first (sum of per-test seconds; these"
            " overlap, so the column is not wall clock):",
            file=stream,
        )
        for row in report.by_binary()[:15]:
            print(
                f"  {row['total_seconds']:>9.1f}s  max {human(row['max_seconds']):>8}"
                f"  n={row['count']:<5} {row['binary'][:38]}",
                file=stream,
            )

    bad = report.bad_statuses()
    if bad:
        print("", file=stream)
        print(f"NON-PASS results ({len(bad)}), all of them:", file=stream)
        for test in bad:
            print(f"  {human(test.seconds):>10}  {test.status:<12} {test.name}", file=stream)

    _render_resources(report, stream, wall)

    print("", file=stream)
    print(
        f"output lines: {report.lines_seen} total"
        f" ({', '.join(f'{k}={v}' for k, v in sorted(report.stream_counts().items()))})"
        f", every line kept with its arrival offset in the JSON",
        file=stream,
    )

    if report.cargo_compile_seconds is not None:
        print("", file=stream)
        print(f"cargo reported compile time: {human(report.cargo_compile_seconds)}", file=stream)
    if report.lock_wait_observed:
        print("cargo reported build-lock contention on this run", file=stream)
    if report.nextest_seconds is not None:
        print(
            f"nextest reported test time   : {human(report.nextest_seconds)}"
            f" over {report.tests_run} tests",
            file=stream,
        )
    print("=" * 78, file=stream)
