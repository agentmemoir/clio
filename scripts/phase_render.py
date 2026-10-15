#!/usr/bin/env python3
"""Printing for the phase timer's report.

`human` renders seconds as a stopwatch figure.
`render` writes the whole breakdown: the stage timeline, the silent gaps, the per-test numbers, the kernel counters, and every non-pass result.
It reads a `Report` and never changes one.
"""

from __future__ import annotations

import sys
from typing import TYPE_CHECKING, TextIO

if TYPE_CHECKING:
    from phase_report import Report


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
