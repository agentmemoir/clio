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
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

# The model, aggregation, and rendering live in phase_report so this file stays
# about running and parsing. Same directory, so the import resolves when the
# Makefile calls `python3 scripts/phase_timer.py`.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from phase_report import (  # noqa: E402
    COUNTER_FIELDS,
    Report,
    Sample,
    TestResult,
    render,
)

# How often to read the kernel's counters for the child's process tree. One
# second is fine enough to attribute a multi-second stall to the interval it
# happened in, and the cost is a handful of `ps` calls per tick.
SAMPLE_INTERVAL_SECONDS = 1.0

# Every stage this timer can name, matched against the lines cargo and nextest
# actually print. Ordered by specificity: the first match wins, so the specific
# patterns come before the general ones.
#
# The list is deliberately exhaustive about the stages that print nothing. A
# silent stage is the expensive kind: it cannot be inferred from the output, so
# if it is not named here it lands in an unnamed gap and someone has to guess
# what it was. That is the specific failure this script exists to prevent.
PHASE_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    # --- blocking on another process -------------------------------------
    (
        "lock-wait",
        re.compile(r"Blocking waiting for (?:file lock|the file lock)"),
    ),
    # --- dependency acquisition ------------------------------------------
    # `cargo update`/fetch prints "Updating `crates-io` index"; older cargo also
    # prints "Updating crates.io index". Anchored loosely so both are caught.
    ("fetch-index", re.compile(r"Updating\s+(?:`[^`]+`|crates\.io)\s+index")),
    ("fetch-lock", re.compile(r"^\s*Locking\s+\d+ packages")),
    ("download", re.compile(r"^\s*Downloading\b")),
    ("download", re.compile(r"^\s*Downloaded\b")),
    # --- workspace resolution --------------------------------------------
    # cargo prints "    Adding serde v1.0.0 to dependency graph" and
    # "    Updating serde v1.0.0 (...)", both of which are the resolver working.
    ("resolve", re.compile(r"^\s*(?:Adding|Removing)\s+\S+\s+v\S+")),
    # --- compilation -------------------------------------------------------
    ("compile", re.compile(r"^\s*(?:Compiling|Building)\b")),
    # cargo prints this when a unit was already valid. Cheap, but it is a real
    # line and its absence is information: no "Fresh" line means real work.
    ("fresh", re.compile(r"^\s*Fresh\b")),
    # --- the build is done; nextest now takes over -------------------------
    ("build-done", re.compile(r"^\s*Finished\b.*profile")),
    # --- test execution ----------------------------------------------------
    ("test-start", re.compile(r"^\s*Starting\s+\d+\s+tests?\b")),
    ("summary", re.compile(r"^\s*Summary\b")),
]

# Phases that mean "the build phase is over". The silence between the build
# finishing and the first test starting is nextest listing every test binary,
# which on this workspace means running ~40 binaries with --list. It prints
# nothing at all, and it used to be filed under "compile-done", which is a name
# that sends a reader looking at the compiler instead of at nextest.
POST_BUILD_PHASES = {"build-done", "fresh"}

# The stage name for that silent span.
LISTING_PHASE = "test-binary listing"

# nextest prints one result per test:
#     PASS [   0.092s] ( 62/3484) clio::bin/clio some::test_name
# A SLOW or TIMEOUT line has extra words between the bracket and the counter.
TEST_RESULT = re.compile(
    r"^\s*(?P<status>PASS|FAIL|SLOW|TIMEOUT|TRY \d+ FAIL|LEAK)\s+"
    r"\[\s*(?P<seconds>[0-9.]+)s\]\s*"
    r"(?:\([^)]*\)\s*)*"
    r"\(\s*(?P<index>\d+)\s*/\s*(?P<total>\d+)\)\s+"
    r"(?P<name>\S.*?)\s*$"
)

# nextest's closing line: "Summary [ 381.319s] 3484 tests run: ..."
SUMMARY = re.compile(
    r"^\s*Summary\s*\[\s*(?P<seconds>[0-9.]+)s\]\s*"
    r"(?P<run>\d+)\s+tests run:\s*(?P<detail>.*)$"
)

# Cargo's own compile total, e.g. "Finished `test` profile ... in 2m 57s".
CARGO_TOTAL = re.compile(r"in\s+(?:(?P<h>\d+)h\s*)?(?:(?P<m>\d+)m\s*)?(?:(?P<s>[0-9.]+)s)")

# A bare "N tests run" tally that is not the Summary line, so a run whose
# summary never printed still records how many tests it believes it ran.
TESTS_RUN_TALLY = re.compile(r"\b(?P<run>\d+)\s+tests?\s+run\b")


def _cargo_total_seconds(line: str) -> float | None:
    """Cargo's self-reported compile time in seconds, if this line carries it."""
    match = CARGO_TOTAL.search(line)
    if not match:
        return None
    hours = float(match.group("h") or 0)
    minutes = float(match.group("m") or 0)
    secs = float(match.group("s") or 0)
    return hours * 3600 + minutes * 60 + secs


def _observe(report: Report, stream: str, text: str, at: float, log=None) -> None:
    """Record one line: phase boundaries, test result, and the line itself.

    Called for every line the child prints, on both streams, in arrival order.
    """
    previous = report.lines[-1] if report.lines else None
    if previous is not None:
        # The interval since the previous line is the runner working silently.
        # Attribute it to whatever phase was open, so process spawn and reaping
        # costs show up as themselves instead of as a residual.
        #
        # One exception: the silence after cargo says the build is finished and
        # before nextest says it is starting tests is nextest enumerating every
        # test binary. That is not the build and not the tests, and filing it
        # under either sends a reader to the wrong program. It gets its own name.
        cause = LISTING_PHASE if previous.phase in POST_BUILD_PHASES else previous.phase
        report.add_gap(f"{cause} (silent)", at - report.started - previous.offset, at)

    if log is not None:
        log.write(text + "\n")
        log.flush()

    current = None
    for phase in reversed(report.phases):
        if phase.end is None:
            current = phase
            break
    if current is None:
        report.add_gap("process startup (before first line)", at - report.started, at)
        current = report.open_phase("startup", at)

    for name, pattern in PHASE_PATTERNS:
        if pattern.search(text):
            if name == "lock-wait":
                report.lock_wait_observed = True
            if name == "build-done":
                seconds = _cargo_total_seconds(text)
                if seconds is not None:
                    report.cargo_compile_seconds = seconds
            current = report.open_phase(name, at)
            break

    result = TEST_RESULT.match(text)
    if result:
        report.tests.append(
            TestResult(
                name=result.group("name"),
                seconds=float(result.group("seconds")),
                status=result.group("status"),
                at=at,
            )
        )
        # Do NOT open a phase per test. Reuse the running "tests" phase so the
        # phase list stays a handful of readable spans instead of one entry per
        # test. Every test is still recorded individually in `report.tests`.
        if current.name != "tests":
            current = report.open_phase("tests", at)
        current.lines += 1
    else:
        current.lines += 1
        summary = SUMMARY.match(text)
        if summary:
            report.nextest_seconds = float(summary.group("seconds"))
            report.tests_run = int(summary.group("run"))
        elif report.tests_run is None:
            tally = TESTS_RUN_TALLY.search(text)
            if tally:
                report.tests_run = int(tally.group("run"))

    report.add_line(at - report.started, stream, text, current.name)


def _read_counters(pid: int) -> tuple[int, dict[str, float]] | None:
    """Read one process's kernel counters, or None if it is gone.

    `ps` is used rather than /proc because this runs on macOS, where /proc does
    not exist. Only the fields this report needs are requested, so the cost per
    sample is one fork per live process at the sampling interval.
    """
    # `readb`, `writeb`, and `rsspeak` are Linux-only ps keywords and are absent
    # on macOS, where asking for them makes ps print a keyword error and no
    # counters at all. Probe the supported set once, then request only those, so
    # one unsupported field cannot silently zero out every measurement.
    global _PS_FIELDS  # noqa: PLW0603
    if _PS_FIELDS is None:
        _PS_FIELDS = _supported_ps_fields()
    if not _PS_FIELDS:
        return None
    done = subprocess.run(  # noqa: S603
        ["ps", "-o", f"{','.join(_PS_FIELDS)}=", "-p", str(pid)],  # noqa: S607
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        return None
    # ps prints a header row even with the `=` suffix, and prints `-` for a
    # counter it cannot collect on this host or for this process. Both are
    # normal, not errors: take the data row, which is the last line whose first
    # token is a number, and read `-` as a zero for that one field rather than
    # discarding the whole reading. Selecting by content rather than position
    # matters because ps right-aligns columns and collapses headers, so token
    # counts do not line up with the requested field count.
    parts: list[str] = []
    for row in reversed(done.stdout.splitlines()):
        tokens = row.split()
        if tokens and _is_number(tokens[0]):
            parts = tokens
            break
    if not parts:
        return None
    # The `time` column is [[dd-]hh:]mm:ss rather than a number, so it is kept
    # as its own string and converted once, below, where it is used.
    time_token = "0"
    values: list[float] = []
    for name, token in zip(_PS_FIELDS, parts[: len(_PS_FIELDS)], strict=False):
        if token == "-":
            values.append(0.0)
        elif name == "time":
            time_token = token
            values.append(0.0)
        else:
            values.append(float(token))
    if len(values) != len(_PS_FIELDS):
        return None
    # `ps` keyword -> this report's counter name. They differ: ps calls resident
    # size `rss` and the fault counts `minflt`/`majflt`, while the report groups
    # them under the names the OS accounting API uses. Reading the ps names
    # straight into the report would file every one of them under a key the
    # report never looks at, so the counters would all read zero.
    reading = {name: 0.0 for name in COUNTER_FIELDS}
    for name, value in zip(_PS_FIELDS, values, strict=True):
        target = PS_TO_COUNTER.get(name)
        if target is not None:
            reading[target] = value
    # macOS `ps` reports one combined time column with no user/system split, so
    # it is charged to system time here and the exact end-of-run figures from the
    # OS accounting API correct the split afterwards. Charging it to system time
    # is the conservative direction: it can overstate kernel cost, never hide it.
    reading["system_seconds"] = _ps_time_seconds(time_token)
    reading["user_seconds"] = 0.0
    return 1, reading


# `ps` keyword -> this report's counter name. They differ: ps calls resident size
# `rss` and the fault counts `minflt`/`majflt`, while the report groups them under
# the names the OS accounting API uses. Without this map every sampled counter
# would be filed under a key the report never reads, and all of them would read
# zero — a measurement that looks valid and says nothing.
PS_TO_COUNTER = {
    "rss": "max_rss_kb",
    "rsspeak": "peak_rss_kb",
    "time": "time",
    "minflt": "minor_faults",
    "majflt": "major_faults",
    "nvcsw": "voluntary_ctx_switches",
    "nivcsw": "involuntary_ctx_switches",
    "readb": "read_bytes",
    "writeb": "write_bytes",
}

_PS_FIELDS: list[str] | None = None


def _is_number(text: str) -> bool:
    try:
        float(text)
    except ValueError:
        return False
    return True


def _supported_ps_fields() -> list[str]:
    """The counter keywords this host's `ps` actually understands.

    A keyword counts as supported only when the probe exits zero. The exit
    status is the signal; the wording on stderr is not. `ps` refuses an unknown
    keyword differently on each platform — macOS says ``readb: keyword not
    found``, procps says ``unknown user-defined format specifier "readb"``, and
    toybox has its own phrasing again — so matching on the message marks a
    keyword supported on a host that cannot print it, and one unsupported field
    then fails every combined read and silently zeroes the whole run's sampling.

    The probe asks about this process rather than pid 1 so a "no such process"
    failure cannot be mistaken for an unsupported keyword: a live pid is a
    precondition for the exit status to mean what it is read to mean.
    """
    wanted = [
        "rss",
        "time",
        "minflt",
        "majflt",
        "nvcsw",
        "nivcsw",
        "readb",
        "writeb",
        "rsspeak",
    ]
    supported: list[str] = []
    for name in wanted:
        probe = subprocess.run(  # noqa: S603
            ["ps", "-o", f"{name}=", "-p", str(os.getpid())],  # noqa: S607
            capture_output=True,
            text=True,
            check=False,
        )
        if probe.returncode == 0:
            supported.append(name)
    return supported


def _ps_time_seconds(field: str) -> float:
    """`ps` time is [[dd-]hh:]mm:ss, all of it charged to user plus system.

    macOS `ps` does not split user from system in the `time` column, so this
    returns the combined figure. That is enough to answer the question the
    report exists to answer (how much was the kernel versus the CPU) at the
    process-tree level, where `times()` provides the split.
    """
    days = 0
    if "-" in field:
        day_part, field = field.split("-", 1)
        days = int(day_part)
    chunks = field.split(":")
    seconds = 0.0
    for chunk in chunks:
        seconds = seconds * 60 + float(chunk)
    return seconds + days * 86400


def _descendants(root: int) -> list[int]:
    """The root pid plus every live descendant, via one `pgrep -P` walk."""
    found = [root]
    frontier = [root]
    while frontier:
        done = subprocess.run(  # noqa: S603
            ["pgrep", "-P", str(frontier.pop())],  # noqa: S607
            capture_output=True,
            text=True,
            check=False,
        )
        for token in done.stdout.split():
            try:
                child = int(token)
            except ValueError:
                continue
            if child not in found:
                found.append(child)
                frontier.append(child)
    return found


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


def _accurate_tree_counters(root: int) -> dict[str, float] | None:
    """Exact user/system split for the tree, from the OS accounting API.

    `ps` cannot split user from system, and that split is the whole question: a
    build can spend three times its CPU time inside the kernel and still print a
    normal-looking log. Falls back to None when `resource` is unavailable, which
    is the path the report names rather than hides.
    """
    try:
        import resource
    except ImportError:  # pragma: no cover - not reachable on macOS or Linux
        return None
    try:
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    except (OSError, ValueError):
        return None
    return {
        "user_seconds": float(usage.ru_utime),
        "system_seconds": float(usage.ru_stime),
        "max_rss_kb": float(usage.ru_maxrss),
        "minor_faults": float(usage.ru_minflt),
        "major_faults": float(usage.ru_majflt),
        "voluntary_ctx_switches": float(usage.ru_nvcsw),
        "involuntary_ctx_switches": float(usage.ru_nivcsw),
        "read_bytes": 0.0,
        "write_bytes": 0.0,
        "peak_rss_kb": float(usage.ru_maxrss),
    }


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
