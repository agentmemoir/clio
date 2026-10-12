#!/usr/bin/env python3
"""Shared harness for the phase-timer suites.

`scripts/test_phase_timer.py` and `scripts/test_phase_timer_sampling.py` both
drive the real timer over a stand-in child and read its JSON report. The harness
that builds that child and reads its output lives here once, so the two suites
cannot drift apart.

This file holds no tests. Run the suites through the entry point:

    python3 scripts/test_phase_timer.py
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

TIMER = SCRIPTS / "phase_timer.py"

# A child's output shaped exactly like cargo + nextest, so the parsing paths are
# exercised without paying for a real 5-minute build.
FAKE_RUN = r"""
echo "    Blocking waiting for file lock on build directory"
echo "   Compiling clio v0.1.0 (/repo/crates/clio)"
echo "    Finished `test` profile [unoptimized + debuginfo] target(s) in 1m 30s"
echo "  Starting 3 tests across 2 binaries (10 tests skipped)"
echo "        PASS [   1.500s] ( 1/3) clio::bin/clio alpha::one"
echo "        PASS [   0.250s] ( 2/3) clio::bin/clio alpha::two"
echo "        FAIL [   0.750s] ( 3/3) clio-store beta::three"
echo "  Summary [   2.500s] 3 tests run: 2 passed, 1 failed"
exit 100
"""


def run_timer(script: str, *extra: str, args: tuple[str, ...] = ()) -> tuple[int, str, dict, str]:
    """Run the timer over `script`; return exit code, stderr report, JSON, raw log."""
    with tempfile.TemporaryDirectory() as tmp:
        json_path = Path(tmp) / "timing.json"
        log_path = Path(tmp) / "raw.log"
        argv = [
            sys.executable,
            str(TIMER),
            "--json",
            str(json_path),
            "--log",
            str(log_path),
            *extra,
            "--",
            "/bin/sh",
            "-c",
            script,
            *args,
        ]
        done = subprocess.run(argv, capture_output=True, text=True, check=False)  # noqa: S603
        report = json.loads(json_path.read_text()) if json_path.exists() else {}
        raw = log_path.read_text() if log_path.exists() else ""
        return done.returncode, done.stderr, report, raw
