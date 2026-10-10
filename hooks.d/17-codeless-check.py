#!/usr/bin/env python3
# precommit-scope: manual
#
# It runs the no-code file check and nothing else. The runner now decides the
# property from each file's own source and keeps the recorded list as a
# cross-check, so this check runs here until the last entries are removed.
"""Hook: every file the coverage gate skips for holding no code still holds none.

`scripts/coverage_codeless.json` records the files whose items are only module
declarations, `use` statements, type aliases and `const` or `static` values, so
they compile to no instrumented line and no coverage number can apply to them.
The runner decides that property from each file's own source and keeps the record
as a cross-check while the last entries are removed.

A recorded file that has grown a function would leave the record stale, so
`scripts/coverage_codeless_check.py` reads every recorded path and fails when one
holds a function or a macro that can expand to one, and when a recorded path no
longer exists.

The hook takes no files. It runs the check and exits with its status, printing
one line when the list is sound and the check's own output when it is not. The
self-test's per-case lines are folded into its verdict, because a manual hook
that prints thirty lines every time trains its reader to skip the one that
matters.

Exit codes: 0 the list is sound, non-zero it is not.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

CHECK = "scripts/coverage_codeless_check.py"


def run(root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(  # noqa: S603
        [sys.executable, str(root / CHECK), *arguments],
        capture_output=True,
        text=True,
        check=False,
    )


def verdict_line(output: str) -> str | None:
    """The self-test's own closing line, which is the only verdict it prints."""
    for line in reversed(output.splitlines()):
        if line.startswith("self-test:"):
            return line
        if line.startswith("self-test FAIL"):
            return line
    return None


def main() -> int:
    root = Path(os.environ.get("CLIO_PRECOMMIT_ROOT", Path.cwd()))

    # The self-test first, and quietly: its failures are its own bugs rather than
    # a finding about the tree, so they are reported as a verdict and not as a
    # list of thirty cases.
    tests = run(root, "--self-test")
    if tests.returncode != 0:
        print(f"CODELESS: {verdict_line(tests.stdout + tests.stderr) or 'self-test failed'}")
        for line in (tests.stdout + tests.stderr).splitlines():
            if line.startswith("self-test FAIL") or line.startswith("codeless-check: FINDING"):
                print(f"CODELESS| {line}")
        return tests.returncode

    checked = run(root)
    output = checked.stdout + checked.stderr
    if checked.returncode != 0:
        for line in output.splitlines():
            print(f"CODELESS| {line}")
        return checked.returncode
    for line in output.splitlines():
        if line.startswith("codeless-check:"):
            print(f"CODELESS: {line.split(':', 1)[1].strip()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())