#!/usr/bin/env python3
"""Check the frozen row that carries the shipped activated skill body.

The row holds one file's measured cl100k_base token count together with the
identity a later comparison needs: the digest of the measured bytes, the digest
of the input the count was computed from, the role the file plays in the shipped
surface, the repo-relative file it measures, and the factor the count is
multiplied by. The module reads that row out of a baseline record and returns
error strings; every string starts with the acceptance criterion it checks, so a
caller can print the list without adding context.

The count is recomputable from public inputs: the measured input is the
in-repo file the row names, its digest is recorded here, and the record's
tokenizer block pins the version and the rank-table digest the count was taken
with. So the row must say ``recomputable`` is true, must carry the recorded input
digest, and any receipt it keeps must resolve and must state that public CI can
recompute the count. A row that says it cannot be recomputed is refused, because
the committed input, the pinned tokenizer, and the committed rank table together
make that claim false.

Nothing here measures and nothing here compares. A caller supplies the record, so
the checks run in the hermetic job without the tokenizer.
"""

ROW_ID = "activated_skill_body/tokens"
UNIT = "tokens"
LABEL = "Measured"
TOKENIZER_REF = "pinned"
CONTENT_SHA256 = "09b61130277acd22823f2ad5d3fd3c5f329df1e34f69bf5c07084b9b0a407866"
#: Digest of the raw bytes the token count was taken from: the same file, so the
#: same bytes, recorded separately under the name recomputability reads it by.
INPUT_SHA256 = CONTENT_SHA256
ROLE = "shipped-activated-skill-body"
FILE = "skills/clio-mcp/SKILL.md"
FACTOR = 1.0

PINNED = (
    ("unit", UNIT),
    ("label", LABEL),
    ("tokenizer_ref", TOKENIZER_REF),
    ("content_sha256", CONTENT_SHA256),
    ("input_sha256", INPUT_SHA256),
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


def _receipt_errors(record, row) -> list:
    """Return one error for a kept receipt that is missing or denies recomputation."""
    receipt_id = row.get("receipt_id")
    receipts = record.get("receipts") if isinstance(record, dict) else None
    receipt = receipts.get(receipt_id) if isinstance(receipts, dict) else None
    if not isinstance(receipt, dict):
        return [f"AC8: row {ROW_ID} receipt_id {receipt_id!r} does not resolve"]
    stated = receipt.get("recomputable_from_public_inputs")
    if stated is not True:
        return [
            f"AC8: row {ROW_ID} receipt {receipt_id!r} states "
            f"recomputable_from_public_inputs is {stated!r}, expected True"
        ]
    return []


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
    recomputable = row.get("recomputable")
    if recomputable is not True:
        errors.append(
            f"AC8: row {ROW_ID} recomputable is {recomputable!r}, expected True: the "
            "recorded input digest and the pinned tokenizer make the count recomputable"
        )
    if "receipt_id" in row:
        errors.extend(_receipt_errors(record, row))
    return errors
