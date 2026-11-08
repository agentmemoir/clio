#!/usr/bin/env python3
"""Check the frozen row that carries the shipped activated skill body.

The row holds one file's measured cl100k_base token count together with the
identity a later comparison needs: the digest of the measured bytes, the role the
file plays in the shipped surface, the repo-relative file it measures, and the
factor the count is multiplied by. The module reads that row out of a baseline
record and returns error strings; every string starts with the acceptance
criterion it checks, so a caller can print the list without adding context.

Nothing here measures and nothing here compares. A caller supplies the record, so
the checks run in the hermetic job without the tokenizer.
"""

ROW_ID = "activated_skill_body/tokens"
UNIT = "tokens"
LABEL = "Measured"
TOKENIZER_REF = "pinned"
CONTENT_SHA256 = "09b61130277acd22823f2ad5d3fd3c5f329df1e34f69bf5c07084b9b0a407866"
ROLE = "shipped-activated-skill-body"
FILE = "skills/clio-mcp/SKILL.md"
FACTOR = 1.0

PINNED = (
    ("unit", UNIT),
    ("label", LABEL),
    ("tokenizer_ref", TOKENIZER_REF),
    ("content_sha256", CONTENT_SHA256),
    ("role", ROLE),
    ("file", FILE),
)


def _is_int(value) -> bool:
    """Return true for a JSON integer and false for a boolean."""
    return isinstance(value, int) and not isinstance(value, bool)


def _rows(record):
    rows = record.get("rows") if isinstance(record, dict) else None
    return [row for row in rows if isinstance(row, dict)] if isinstance(rows, list) else []


def rows(record) -> list:
    """Return every row of ``record`` that carries the activated row id."""
    return [row for row in _rows(record) if row.get("row") == ROW_ID]


def _field_errors(row) -> list:
    """Return one error for each pinned field the row states differently."""
    return [
        f"AC7: row {ROW_ID} {key} is {row.get(key)!r}, expected {value!r}"
        for key, value in PINNED
        if row.get(key) != value
    ]


def check(record) -> list:
    """Return every error in the record's shipped activated skill body row."""
    entries = rows(record)
    if len(entries) != 1:
        return [f"AC7: expected exactly one {ROW_ID} row, found {len(entries)}"]
    row = entries[0]
    errors = _field_errors(row)
    value = row.get("value")
    if not _is_int(value) or value <= 0:
        errors.append(f"AC7: row {ROW_ID} value is not a positive integer: {value!r}")
    factor = row.get("factor")
    if type(factor) is not float or factor != FACTOR:
        errors.append(f"AC7: row {ROW_ID} factor is not the number {FACTOR!r}: {factor!r}")
    return errors
