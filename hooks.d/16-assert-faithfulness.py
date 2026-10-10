#!/usr/bin/env python3
# precommit-scope: manual
#
# It reads the assertion changes and nothing else. It runs no build and no test,
# so its cost does not grow with the size of a commit. It is manual because the
# contract it protects is between what a change claims and what it asserts, which
# is not what a staged file list is asked about -- a commit hook that judged only
# the staged file would miss an uncommitted edit the branch then carried.
"""Hook: a mechanical pass over assertions changed only what it was asked to.

One toolchain move made a lint an error, and the fix was mechanical: a script
added a failure message to every emptiness assertion, over three hundred of them
across a hundred and seventy-three files. Three defects came in, and every one
looked like finished work.

An assertion was invented at a line no lint had ever flagged, and it failed on
every run because the field it asserted on is a budget of one call and a spent
budget is correct behaviour. An expression was evaluated twice: where the
condition was not bound to a `let` first, the message argument called it again,
so a clock read printed a number nobody asserted on and a row-writing helper ran
twice. Sixty-two identifiers were corrupted by a rewrite that stripped three
characters where it meant two.

None of the three was visible in the diff. All three were found by running
something afterwards, which is this check.

The hook takes no files. It runs the check and exits with its status. The check
reads the merge base against `origin/master`, so a commit on a branch that has
never fetched is refused rather than judged against nothing.

Exit codes: 0 every assertion still asserts the same thing, 1 a finding, 2 a
usage error, 3 the base does not resolve or a file could not be read.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

CHECK = "scripts/assert_faithfulness_check.py"


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
        if line.startswith("assert-faithfulness:"):
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