#!/usr/bin/env python3
# precommit-scope: manual
#
# It runs this runner's own test suite and reads nothing about the commit. It
# is also the one hook whose subject is the commit check itself, so running it
# from inside the commit check would test the thing making the decision while
# it decides. It runs when it is asked for by name, and in CI.
"""Hook: the pre-commit runner still discovers and fails the way it should.

`scripts/precommit.py` decides whether a set of checks ran. Every failure mode
it has in common is one where it reports success while a check silently did not
run: a hook whose file mode was lost, a hook link whose target moved, an
unknown `--only` name, an empty file list. That is worse than having no runner,
because the reader cannot tell the difference.

The `scripts/test_precommit*.py` suites test exactly those paths, each case built
against a throwaway git repository so the answer is the same on any machine.
That last part is the repository carrying a git configuration of its own: a
throwaway repository still reads the machine's identity, hook directory and
signing setting otherwise, and a suite that inherits those gives a different
answer on a build machine than on a laptop.

This hook takes no files. It costs a little over forty seconds on a development
machine. CI runs the same suite as a step of its own, under
`make scripts-check-precommit`, so a broken runner fails
a pull request rather than waiting to be run by hand.

Exit codes: 0 the runner is sound, non-zero it is not.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

TARGET = "scripts-check-precommit"


def main() -> int:
    root = Path(os.environ.get("CLIO_PRECOMMIT_ROOT", Path.cwd()))
    done = subprocess.run(  # noqa: S603
        ["make", "-C", str(root), TARGET],
        capture_output=True,
        text=True,
        check=False,
    )
    output = done.stdout + done.stderr
    # The tail carries the summary and the failure detail; the per-test names
    # above it are noise once everything has passed.
    for line in output.splitlines()[-40:]:
        print(line)
    return done.returncode


if __name__ == "__main__":
    sys.exit(main())
