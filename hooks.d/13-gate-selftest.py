#!/usr/bin/env python3
# precommit-scope: manual
#
# It runs the gate wrapper's self-test and nothing else, and CI runs it too. A
# commit hook should read what is being committed; this reads nothing and repeats
# something the continuous integration run already covers.
"""Hook: the gate still gives a stored verdict only to the command it measured.

`scripts/gate.py` runs a heavy command at most once per working tree and hands
the stored exit code and log back to anything that asks for it again. Every
tool in a checkout goes through it, including the pre-commit hooks themselves,
because the shim installs itself as `cargo` and `make` on the path. So when it
answers with the wrong verdict, the failure lands on an unrelated check and the
message names a command nobody ran.

The wrapper classifies what it is asked to run, and that classification decides
which log a verdict is published under and whether two commands may share an
entry. The self-test covers both, including the shape where two commands differ
only in an environment variable the scoping code reads, which the cache key used
to ignore. Nothing ran it before this hook existed: not the commit hook, not
`make precommit`, and not CI, so a regression here would have been found by a
reader rather than by a check.

The hook takes no files. It runs the self-test and exits with its status.

Exit codes: 0 the gate is sound, non-zero it is not.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

GATE = "scripts/gate.py"


def main() -> int:
    root = Path(os.environ.get("CLIO_PRECOMMIT_ROOT", Path.cwd()))
    done = subprocess.run(  # noqa: S603
        [sys.executable, str(root / GATE), "--self-test"],
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
        print(f"{GATE}: exited {done.returncode} without printing a self-test verdict")
        for line in output.splitlines():
            print(f"{GATE}| {line}")
    return done.returncode


if __name__ == "__main__":
    sys.exit(main())
