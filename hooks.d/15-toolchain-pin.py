#!/usr/bin/env python3
# precommit-scope: manual
#
# It reads the pin file and the workflows, and nothing else. It builds nothing, so
# its cost does not grow with the size of a commit, and it is manual because the
# contract it protects is between the runner and the compiler rather than between
# one file and another, which is not what a commit hook is asked about.
"""Hook: every workflow installs the toolchain the repository pins.

`rust-toolchain.toml` names one channel, and each workflow also passes that
version to the install action explicitly, because the action's own documentation
says an explicit input can override the toolchain file. Naming it in both places
removes the question of which wins, and creates one to keep: raising the pin
without raising the workflow leaves the runner compiling, linting and measuring
coverage with a different compiler than the repository claims to use.

The workflow already has a step that fails when the runner and the pin disagree.
That step runs on the runner, so it answers after the push. This one answers
before it, and names the file and line rather than two versions.

The hook takes no files. It runs the check and exits with its status.

Exit codes: 0 every installed toolchain equals the pin, 1 a finding, 2 a usage
error, 3 a file it had to read could not be read.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

CHECK = "scripts/toolchain_pin_check.py"


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
        if line.startswith("toolchain-pin:"):
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
