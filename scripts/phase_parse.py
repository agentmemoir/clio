#!/usr/bin/env python3
"""Parse the child's output into report events.

Every line cargo or nextest prints is matched here against the known stage patterns.
A test-result line becomes a `TestResult`, and a summary or tally line records how many tests the child ran.
The report object is owned by `phase_report`; this module only reads text.
"""

from __future__ import annotations

import re

from phase_report import Report, TestResult


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
