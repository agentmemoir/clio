#!/usr/bin/env python3
# precommit-scope: manual
#
# It runs a 42-test suite and reads nothing about the commit, so it is left out
# of the commit run on purpose: charging every commit eleven seconds for a check
# about the timing harness is how a commit hook gets passed with --no-verify.
"""Hook: the phase timer still measures what it claims to measure.

`scripts/phase_timer.py` attributes every second of a test run to a named
cause. Its first version measured a run correctly and then discarded the
measurement: it wrote none of the child's output, kept only the ten slowest
tests, and printed no failing test. `scripts/test_phase_timer.py` is 42 tests
that pin those losses shut.

Nothing ran that suite. It was not in the Makefile and not in CI, so the guard
against silent evidence loss was itself unguarded: a change to the timer could
have removed a guarantee and the suite would have said nothing until someone
read it. Both gaps are closed now. `make scripts-check-phase-timer` runs it, and
CI runs that target as a step of its own, so a broken suite fails a pull request
instead of waiting to be read.

This hook takes no files. It costs about 11 seconds, so it is a separate hook
from the fast ones and can be skipped while iterating:

    python3 scripts/precommit.py --skip 20-phase-timer-tests

Exit codes: 0 the timer still measures correctly, non-zero it does not.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

SUITE = "scripts/test_phase_timer.py"


def main() -> int:
    root = Path(os.environ.get("CLIO_PRECOMMIT_ROOT", Path.cwd()))
    done = subprocess.run(  # noqa: S603
        [sys.executable, str(root / SUITE)],
        capture_output=True,
        text=True,
        check=False,
    )
    output = done.stdout + done.stderr
    # Only the tail: the suite prints a line per test and the failure detail is
    # what a reader needs, not 42 passing names.
    for line in output.splitlines()[-40:]:
        print(line)
    return done.returncode


if __name__ == "__main__":
    sys.exit(main())
