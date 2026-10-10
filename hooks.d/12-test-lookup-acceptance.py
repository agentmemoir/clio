#!/usr/bin/env python3
# precommit-scope: manual
#
# It checks the test lookup against the test runner's own list of tests, and
# nothing else. The lookup decides which tests run when a file changes. A lookup
# that skips a test cannot report that it did, so the run still comes back green
# having executed less than it claims. That is not a hypothetical: two earlier
# attempts at this failed exactly this way in this repository, and both reported
# success while skipping real test modules.
"""Hook: every test module the runner reports is claimed by some source file.

`scripts/owned_tests.py` reads a file's own `mod` declarations to answer "which
tests belong to this file". Its correctness cannot be checked against its own
reasoning, because a lookup that assumes the wrong rule passes whenever that rule
is wrong. So it is checked against `cargo nextest list`, which is the only thing
that knows what actually compiles.

The check builds the workspace's test binaries to compare against, so its cost is
not bounded by the size of a commit and it is manual for that reason: a commit
does not run it, and `make precommit` does. It prints one line per unclaimed
module, and fails if there is any.

This hook takes no files. It runs the acceptance check and exits with its status.

Exit codes: 0 every test module is claimed, non-zero one is not.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

LOOKUP = "scripts/owned_tests.py"


def main() -> int:
    root = Path(os.environ.get("CLIO_PRECOMMIT_ROOT", Path.cwd()))
    done = subprocess.run(  # noqa: S603
        [sys.executable, str(root / LOOKUP), "--accept"],
        capture_output=True,
        text=True,
        check=False,
    )
    text = done.stdout + done.stderr
    for line in text.splitlines():
        if "UNCLAIMED" in line or line.startswith("acceptance:"):
            print(line)
    return done.returncode


if __name__ == "__main__":
    sys.exit(main())