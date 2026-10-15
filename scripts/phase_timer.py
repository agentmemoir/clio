#!/usr/bin/env python3
"""Time every phase of a cargo/nextest run by timestamping its output as it arrives.

Why this exists
---------------
On 2026-09-28 a full-workspace `make test` took 958.8s. The log said the tests
themselves took 381.3s and the compile took 2m57s. That left 400s unexplained,
and I reported the remainder as "build lock waiting" by subtracting one number
from another. The log printed `Blocking waiting for file lock on build
directory` on its second line, which made the guess look measured. It was not.
I had no timestamp on any line, so every phase boundary was a guess.

What was still missing afterwards
---------------------------------
The first version of this script fixed that incident and then created a second
one. It read every line the child printed and threw the line away, kept only the
ten slowest tests in its JSON, and never printed a failing test. A later run
exited 100 with real failures and the report showed ten passing tests and no
failure list, so the failing names were unrecoverable. Measuring a run and then
discarding the measurement is the same mistake as never having measured it.

So nothing is sampled any more:

- Every output line from both streams is kept with its arrival offset.
- Every individual test result is kept, not a top-ten summary.
- Every silent interval between lines is kept individually, so a long single
  stall cannot hide inside an average.
- Every non-pass result is printed in full, and the run's exit code is
  reconciled against them: a non-zero exit with no captured failure says so
  rather than passing silently.
- The child's output is streamed through live, so the log is never an empty
  file while a run is in progress. An empty log means "still going"; it must
  never mean "nothing was recorded".
- When nextest reports more tests than this script saw result lines for, the
  report says so and names the cause (the runner was not asked to print every
  result), instead of quietly reporting a partial picture as a whole one.

Usage
-----
    python3 scripts/phase_timer.py -- make test
    python3 scripts/phase_timer.py --json /tmp/timing.json -- make test
    python3 scripts/phase_timer.py --log /tmp/run.log --json /tmp/timing.json -- make test

To capture every per-test timing, the wrapped command must print every result.
nextest does that with `--status-level all`:

    python3 scripts/phase_timer.py -- cargo nextest run --status-level all

It proxies the child's exit code, so it can stand in for the command in a
pipeline or a Makefile.

Environment:
    PHASE_TIMER_JSON   write the timing report to this path as JSON
    PHASE_TIMER_LOG    write the child's raw output to this path
"""


from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

# The runner is the only executable entry point. It reaches parsing, the model,
# and the sampler through their own modules, all in this directory so the import
# resolves when the Makefile calls `python3 scripts/phase_timer.py`.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from phase_parse import _observe  # noqa: E402
from phase_report import COUNTER_FIELDS, Report, Sample, render  # noqa: E402
from phase_sampler import (  # noqa: E402
    SAMPLE_INTERVAL_SECONDS,
    _accurate_tree_counters,
    _descendants,
    _read_counters,
    _PS_FIELDS,  # noqa: F401 - re-exported for the sampling tests
    _supported_ps_fields,  # noqa: F401 - re-exported for the sampling tests
)


def _sample_tree(report: Report, root: int) -> None:
    """Take one reading of every live process in the child's tree."""
    totals = {name: 0.0 for name in COUNTER_FIELDS}
    live = 0
    seen = 0
    for pid in _descendants(root):
        seen += 1
        reading = _read_counters(pid)
        if reading is None:
            continue
        live += 1
        for name, value in reading[1].items():
            # A reader may carry a key the report does not track; summing into
            # the counter dict with a default keeps one extra field from killing
            # the whole sampler thread and losing every later sample with it.
            totals[name] = totals.get(name, 0.0) + value
    if live == 0:
        # Processes were found and none of them could be read. That is a broken
        # reader, not a run that ended too fast to be sampled, and the two must
        # not read the same in the report: without this the coverage note below
        # claims the tree was never observed live, which is a statement about the
        # run's length and is simply false here.
        if seen:
            report.resources.note_error(
                f"every read of the {seen} live process(es) in the tree failed, so the "
                "sampler observed nothing; this host's `ps` may not support the "
                "requested counter keywords"
            )
        return
    report.resources.add(
        Sample(at=time.time() - report.started, children=live, totals=totals)
    )


def _sample_loop(report: Report, root: int, interval: float, stop: "threading.Event") -> None:
    """Sample the process tree until the run ends.

    A daemon thread, so a hung child cannot leave the sampler running: the
    thread dies with the interpreter. Sampling continues past the child's last
    output line so teardown cost is attributed too.
    """
    report.resources.interval_seconds = interval
    # The first sample is taken before the first wait, so a run shorter than one
    # interval still records the tree it had. Every tick walks the whole tree, so
    # a build that forks 40 linkers is measured 40 times over rather than once.
    # A failure on one tick is recorded and the loop continues: a sampler that
    # dies on the first odd process reports "no samples" for the whole run, which
    # reads as a clean measurement rather than a broken one.
    while True:
        try:
            _sample_tree(report, root)
        except Exception as error:  # noqa: BLE001 - a sampler must not die silently
            report.resources.note_error(f"{type(error).__name__}: {error}")
        if stop.is_set():
            return
        stop.wait(interval)


def _account_for_silent_work(report: Report, last_line_at: float, exited_at: float) -> None:
    """Name the two spans that print nothing, so neither becomes a mystery.

    The gap between the last result line and the child's exit is nextest writing
    its JUnit report and reaping test processes. The gap between this timer's own
    start and the child being spawned is the gate, the shim, and dynamic linking.
    Both are real wall-clock cost; both used to be invisible.
    """
    if last_line_at > report.started:
        report.add_gap("teardown (silent)", exited_at - last_line_at, exited_at)
    if report.spawn_seconds > 0:
        report.add_gap("timer-to-spawn (silent)", report.spawn_seconds, report.started + report.spawn_seconds)


def _run(command: list[str], report: Report, log=None) -> None:
    """Run the child, timestamping every line from both streams as it arrives."""
    env = dict(os.environ)
    # Force unbuffered/line-oriented output from the child so lines arrive as
    # they are produced rather than in a block at exit. Without this the whole
    # measurement collapses into one span and every phase boundary is fiction.
    env.setdefault("CARGO_TERM_COLOR", "never")

    spawned = time.time()
    process = subprocess.Popen(  # noqa: S603
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        env=env,
    )
    assert process.stdout is not None
    # Time from our own start to the child being spawned, and from the last
    # output line to the child exiting. Neither prints a line, so without these
    # two spans the residual looks like an unexplained gap and invites a guess.
    report.spawn_seconds = spawned - report.started

    # Sample the kernel's counters for the whole run, on a daemon thread, so the
    # child's own output is not the only evidence that survives. A build can
    # spend most of its time in the kernel and print an ordinary-looking log;
    # these samples are the only thing that shows it.
    stop = threading.Event()
    sampler = threading.Thread(
        target=_sample_loop,
        args=(report, process.pid, SAMPLE_INTERVAL_SECONDS, stop),
        daemon=True,
    )
    sampler.start()

    for raw in process.stdout:
        _observe(report, "child", raw.rstrip("\n"), time.time(), log)

    last_line_at = time.time()
    report.close_phase(last_line_at)
    report.exit_code = process.wait()
    exited_at = time.time()
    stop.set()
    sampler.join(timeout=SAMPLE_INTERVAL_SECONDS * 2 + 1.0)

    # The exact user/system split for the finished tree. `ps` cannot provide it,
    # so this is what makes "the kernel cost more than the CPU" a measurement
    # rather than an inference from a combined column.
    exact = _accurate_tree_counters(process.pid)
    if exact is not None:
        report.resources.exact = exact
    elif report.resources.unavailable_reason is None:
        report.resources.unavailable_reason = (
            "the OS user/system accounting API was unavailable, so kernel time is"
            " reported combined with user time"
        )
    # nextest writes its JUnit report and tears down its test processes after
    # the last result line, so the tail after that line is measured, not assumed.
    report.exit_seconds = exited_at - last_line_at
    _account_for_silent_work(report, last_line_at, exited_at)


def main(argv: list[str]) -> int:
    args = argv[1:]
    json_path = os.environ.get("PHASE_TIMER_JSON", "")
    log_path = os.environ.get("PHASE_TIMER_LOG", "")
    command: list[str] = []
    usage = f"usage: {argv[0]} [--json PATH] [--log PATH] -- CMD ARGS..."

    # Everything after `--` belongs to the child, including things that look
    # like this script's own flags. Without that, `phase_timer.py -- make test
    # --json x` would silently swallow the child's arguments.
    if "--" in args:
        separator = args.index("--")
        flags, command = args[:separator], args[separator + 1 :]
    else:
        flags, command = args, []

    while flags:
        arg = flags.pop(0)
        if arg == "--json" and flags:
            json_path = flags.pop(0)
        elif arg == "--log" and flags:
            log_path = flags.pop(0)
        else:
            print(usage, file=sys.stderr)
            return 2
    if not command:
        print(usage, file=sys.stderr)
        return 2

    handle = None
    if log_path:
        try:
            Path(log_path).parent.mkdir(parents=True, exist_ok=True)
            handle = open(log_path, "w", encoding="utf-8")  # noqa: SIM115
        except OSError as exc:
            print(f"phase_timer: cannot write {log_path}: {exc}", file=sys.stderr)
            handle = None

    report = Report(command=command, started=time.time())
    try:
        _run(command, report, handle)
    finally:
        if handle is not None:
            handle.close()
    report.ended = time.time()
    render(report)

    if json_path:
        try:
            Path(json_path).parent.mkdir(parents=True, exist_ok=True)
            Path(json_path).write_text(
                json.dumps(report.to_dict(), indent=2) + "\n", encoding="utf-8"
            )
        except OSError as exc:
            print(f"phase_timer: cannot write {json_path}: {exc}", file=sys.stderr)
    return report.exit_code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
