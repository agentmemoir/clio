#!/usr/bin/env python3
# precommit-scope: manual
#
# It runs the scoped runner's self-test and nothing else. The runner picks which
# packages to measure and which files to judge, so a change that broke either
# rule would report a green gate over files it never measured. That is the exact
# failure this tool exists to prevent, so its own tests run here rather than
# waiting for someone to use it and discover the gap.
"""Hook: the scoped runner still selects and judges correctly.

`scripts/scoped_runner.py` is the gate CI runs. It maps changed files to their
owning packages, adds every package that depends on them, runs one instrumented
pass over that set, and judges only the changed files against the one line
percentage and the one number in that file. Three of its rules are load-bearing
and none is visible from reading a diff: a changed file that the report does not
contain must be called unmeasured rather than passed, a file's function coverage
must never be compared with anything, and a base ref that does not resolve must
be an error rather than an empty scope. A break in any of them would turn the
gate into a green run over nothing, or into a second coverage rule.

The hook takes no files. It runs the self-test and exits with its status.

Exit codes: 0 the runner is sound, non-zero it is not.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

RUNNER = "scripts/scoped_runner.py"


def main() -> int:
    root = Path(os.environ.get("CLIO_PRECOMMIT_ROOT", Path.cwd()))
    done = subprocess.run(  # noqa: S603
        [sys.executable, str(root / RUNNER), "--self-test"],
        capture_output=True,
        text=True,
        check=False,
    )
    output = done.stdout + done.stderr
    reported = False
    for line in output.splitlines():
        if line.startswith("self-test FAIL") or "self-test:" in line:
            print(line)
            reported = True
    if done.returncode != 0 and not reported:
        # An exit status on its own sends the reader to the wrong place. The one
        # failure this filter can hide is the self-test dying partway through and
        # printing a traceback, which carries neither pattern above, and the
        # result is a hook that fails with nothing on screen to act on. So when
        # there is no verdict to report, the output that caused it is reported.
        print(f"RUNNER: exited {done.returncode} without printing a self-test verdict")
        for line in output.splitlines():
            print(f"RUNNER| {line}")
    return done.returncode


if __name__ == "__main__":
    sys.exit(main())
