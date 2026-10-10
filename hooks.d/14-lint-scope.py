#!/usr/bin/env python3
# precommit-scope: manual
#
# It reads the build machinery and the workflow, and nothing else. It does no
# build, so its cost does not grow with the size of a commit, but it is manual
# because it describes a contract that only the pull request workflow and the
# local gate depend on, and neither a staged file list nor a commit hook is the
# place a reader looks for it.
"""Hook: no clippy run can select fewer packages than the file says it does.

`make ritual` and the pull request workflow both describe their clippy run as
workspace-wide. Both used to name no package selector, and the root manifest set
`default-members` to a single crate, so cargo selected that one crate for both:
sixteen of the seventeen crates, and their test targets, were never linted by
either, and a `-D warnings` error in any of them could not fail either gate.

Nothing else in the tree would notice that. The scoped test and coverage steps
run the crates a diff touches and report on those, so a crate that is never
compiled is invisible to them rather than wrong. This check reads the three
places the scope is written -- the Makefile, the workflow, and the root manifest
-- and fails when a run could mean a subset without saying so.

The hook takes no files. It runs the check and exits with its status.

Exit codes: 0 every clippy run names its scope and every toolchain matches the
pin, 1 a finding, 2 a usage error, 3 a file it had to read could not be read.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

CHECK = "scripts/lint_scope_check.py"


def main() -> int:
    root = Path(os.environ.get("CLIO_PRECOMMIT_ROOT", Path.cwd()))
    done = subprocess.run(  # noqa: S603
        [sys.executable, str(root / CHECK)],
        capture_output=True,
        text=True,
        check=False,
    )
    output = done.stdout + done.stderr
    reported = False
    for line in output.splitlines():
        if line.startswith("lint-scope-check:"):
            print(line)
            reported = True
    if done.returncode != 0 and not reported:
        # An exit status on its own sends the reader to the wrong place. The one
        # failure this filter can hide is the check dying partway through and
        # printing a traceback, which carries none of the lines above, so the
        # output that caused it is reported rather than swallowed.
        print(f"{CHECK}: exited {done.returncode} without printing a verdict")
        for line in output.splitlines():
            print(f"{CHECK}| {line}")
    return done.returncode


if __name__ == "__main__":
    sys.exit(main())
