#!/usr/bin/env python3
"""Enforce the frozen token ceiling of the shipped activated skill body.

The ceiling comes from one frozen row, read again on every call: the row's own
value multiplied by its factor, which must be exactly ``1.0``. A row that is
missing, duplicated, non-positive, measured in another unit, or that states any
other factor is refused with a name instead of being replaced by a default.

The candidate is compared by token count alone. A byte change or a digest change
that leaves the count where it was still passes, because the bound is token
growth and not content identity; the frozen digest and the candidate digest are
reported side by side for a reader rather than enforced.

Nothing here writes. The record is only read, so measuring a larger candidate can
never move a later ceiling.
"""

from pathlib import Path

import activated_row
from offline_tokenizer import BudgetError

NOT_MEASURED = "not-measured"
NO_PATH = "no activated skill body measured"

#: The shipped body, resolved from the repository root so any cwd measures it.
DEFAULT_PATH = Path(__file__).resolve().parents[2] / activated_row.FILE


def frozen_ceiling(record) -> dict:
    """Return the frozen row's identity and its ceiling, refusing any other row."""
    errors = activated_row.check(record)
    if errors:
        raise BudgetError("budget: " + "; ".join(errors))
    row = activated_row.rows(record)[0]
    value = row["value"]
    factor = row["factor"]
    return {
        "row": activated_row.ROW_ID,
        "file": activated_row.FILE,
        "value": value,
        "factor": factor,
        "ceiling": int(value * factor),
        "sha256": row["content_sha256"],
    }


def not_measured(record) -> dict:
    """Return the ``activated`` object for a report that carries no measurement."""
    frozen = frozen_ceiling(record)
    return {
        "status": NOT_MEASURED,
        "reason": NO_PATH,
        "row": frozen["row"],
        "file": frozen["file"],
        "baseline": frozen["value"],
        "factor": frozen["factor"],
        "ceiling": frozen["ceiling"],
        "failures": [],
    }


def check(record, candidate) -> dict:
    """Compare one measured body with the record's frozen ceiling."""
    frozen = frozen_ceiling(record)
    tokens = candidate["tokens"]
    excess = tokens - frozen["ceiling"]
    result = {
        "status": "measured",
        "row": frozen["row"],
        "file": frozen["file"],
        "baseline": frozen["value"],
        "factor": frozen["factor"],
        "ceiling": frozen["ceiling"],
        "sha256": candidate["sha256"],
        "bytes": candidate["bytes"],
        "tokens": tokens,
        "frozen_sha256": frozen["sha256"],
        "digest_matches": candidate["sha256"] == frozen["sha256"],
        "delta": excess,
        "verdict": "pass",
        "failures": [],
    }
    if excess > 0:
        failure = (
            f"{frozen['row']}: candidate {tokens} exceeds baseline "
            f"{frozen['ceiling']} by {excess}"
        )
        result["verdict"] = "fail"
        result["failures"] = [failure]
    return result
