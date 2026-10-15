#!/usr/bin/env python3
"""Data model for the phase timer's measurements.

Owns the dataclasses every other phase module reads: one counter sample, the resource totals built from samples, and the lines, gaps, phases, and test results observed in the child's output.
Aggregation lives in `phase_report` and printing in `phase_render`; this file holds only the shapes.
"""

from __future__ import annotations

from dataclasses import dataclass, field


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
