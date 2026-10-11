"""What every part of the scoped runner shares: the repository root, the two imports it cannot run without, the one coverage number, exit codes and result types."""

from __future__ import annotations

import sys
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

ROOT = Path(__file__).resolve().parent.parent

sys.path.insert(0, str(Path(__file__).resolve().parent))
try:
    import owned_tests as owned
except ImportError as error:  # pragma: no cover - only when the file is missing
    print(f"scoped-runner: cannot import scripts/owned_tests.py: {error}", file=sys.stderr)
    raise

try:
    from executable_code import offenders
except ImportError as error:  # pragma: no cover - only when the file is missing
    print(
        f"scoped-runner: cannot import scripts/executable_code.py: {error}",
        file=sys.stderr,
    )
    raise

OwnedLookupError = owned.ResolutionError

LINE_FLOOR = 80.0
"""The one coverage number in this repository.

A single fixed percentage, applied to every Rust file a change created or
modified, with no per-file override and no flag that changes it. It was a
per-file recorded floor once, and a per-file recorded floor is two rules: the
fixed number for most files and a second, lower number for whoever's file was
recorded. Recording one also stored the denominator it was measured over, which
made a file's floor depend on which report produced it, so the same tree could
pass on one machine and fail on another.

The metric is line coverage and only line coverage. Function and region
percentages are measured and printed, and neither is compared against this
number or anything else. There is no aggregate, no regression baseline, and no
second gate on the lines a diff added.
"""
COV_DIR = ROOT / "target" / "coverage"
# Its own report path, deliberately NOT the full gate's `coverage.json`.
# Sharing one path meant a fast run silently overwrote the artifact the whole
# report produced, so a later read of `coverage.json` saw a partial tree and
# reported it as if it were the workspace.
REPORT = COV_DIR / "coverage-scoped.json"

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2
EXIT_RUN_ERROR = 3


class RunError(Exception):
    """The measurement could not be produced, so no verdict can be given."""


class CoverageRow(NamedTuple):
    """One file's row: its line percentage, then its function percentage.

    Both percentages are read because the report carries both and a report that
    dropped one of them would be a different measurement. Only the line half is
    judged, by `judge`, and the function half is printed beside it wherever a
    line percentage is printed. Nothing compares a function percentage against
    anything.

    The counts are read for the same reason and judged by nothing. They were what
    a per-file recorded floor was compared against, and that rule is gone.
    """

    lines_pct: float
    functions_pct: float
    lines_count: int | None = None
    functions_count: int | None = None
    lines_covered: int | None = None
    functions_covered: int | None = None
