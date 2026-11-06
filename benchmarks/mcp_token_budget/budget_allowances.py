#!/usr/bin/env python3
"""Load, validate, and resolve the ordered MCP growth-allowance decisions.

An allowance record is the only way a deliberate tool-surface growth can pass the
budget check. Each record names exactly one candidate digest and an independent
ceiling for canonical bytes and for cl100k_base tokens, and it carries a `label`
naming how those ceilings were obtained, so a growth that was reviewed can never
be mistaken for an unreviewed one. A record never names its landing commit: that
commit comes from git history, and a record that carries a commit field is
malformed.

A record file with no records is valid and means no growth is allowed. The module
is stdlib only, so the hermetic job can validate the file without the tokenizer.

Every error is one line prefixed ``allowance:`` and names the rule it broke.
"""

import json
import re
from pathlib import Path

FORMAT = "clio_mcp_growth_allowances"
FORMAT_VERSION = 1

TOP_LEVEL = frozenset({"format", "format_version", "baseline", "records"})
BASELINE_KEYS = frozenset({"path", "sha256"})
RECORD_KEYS = (
    "decision_id",
    "date",
    "reason",
    "base_commit",
    "candidate_sha256",
    "label",
    "max_bytes",
    "max_tokens",
)
RECORD_KEY_SET = frozenset(RECORD_KEYS)
CEILING_KEYS = frozenset({"max_bytes", "max_tokens"})
COMMIT_KEYS = ("commit", "landing_commit", "containing_commit")

#: The labels a record may carry, the same six the baseline rows use.
LABELS = frozenset(
    {"Measured", "Vendor-measured", "External", "Inferred", "Decision", "Unverified"}
)

DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
PRIVATE_SEGMENT_RE = re.compile(r"(^|[\\/])private[\\/]")

#: The two independent measures a record carries a ceiling for, in report order.
MEASURES = ("bytes", "tokens")

#: The record list that means no growth is allowed. Never mutate it.
NONE = []


class AllowanceError(ValueError):
    """Raised when a growth-allowance file cannot be trusted."""


def _fail(message):
    raise AllowanceError(f"allowance: {message}")


def _is_int(value) -> bool:
    """Return true for a JSON integer and false for a boolean."""
    return isinstance(value, int) and not isinstance(value, bool)


def _is_hex(value, pattern) -> bool:
    return isinstance(value, str) and bool(pattern.match(value))


def _forbidden_path(text) -> bool:
    """Return true for an absolute, parent-escaping, or private baseline path."""
    return (
        text.startswith("/")
        or "\\" in text
        or bool(PRIVATE_SEGMENT_RE.search(text))
        or ".." in text.split("/")
    )


def _key_message(where, keys, allowed) -> str:
    missing = sorted(allowed - keys)
    unexpected = sorted(keys - allowed)
    parts = []
    if missing:
        parts.append(f"missing {missing}")
    if unexpected:
        parts.append(f"unexpected {unexpected}")
    return f"{where} keys must be exactly {sorted(allowed)}: " + ", ".join(parts)


def _label(item, index) -> str:
    decision_id = item.get("decision_id")
    if isinstance(decision_id, str) and decision_id.strip():
        return f"record {decision_id!r}"
    return f"record {index}"


def _baseline(value) -> dict:
    if not isinstance(value, dict):
        _fail(f"baseline must be an object, got {type(value).__name__}")
    keys = set(value)
    if keys != BASELINE_KEYS:
        _fail(_key_message("baseline", keys, BASELINE_KEYS))
    digest = value["sha256"]
    if not _is_hex(digest, SHA256_RE):
        _fail(f"baseline sha256 is not 64 lowercase hex: {digest!r}")
    path = value["path"]
    if not isinstance(path, str) or not path:
        _fail(f"baseline path is empty: {path!r}")
    if _forbidden_path(path):
        _fail(f"baseline path is not repo-relative: {path!r}")
    return {"path": path, "sha256": digest}


def _record(item, index) -> dict:
    if not isinstance(item, dict):
        _fail(f"record {index} must be an object, got {type(item).__name__}")
    label = _label(item, index)
    for key in COMMIT_KEYS:
        if key in item:
            _fail(
                f"{label} carries a {key!r} field: the landing commit comes from "
                "git history, never from a record"
            )
    keys = set(item)
    if keys != RECORD_KEY_SET:
        message = _key_message(label, keys, RECORD_KEY_SET)
        if (RECORD_KEY_SET - keys) & CEILING_KEYS:
            message += (
                "; a reason alone is not an allowance: both max_bytes and "
                "max_tokens are required"
            )
        _fail(message)
    decision_id = item["decision_id"]
    if not isinstance(decision_id, str) or not decision_id.strip():
        _fail(f"{label} decision_id is empty")
    date_value = item["date"]
    if not isinstance(date_value, str) or not DATE_RE.match(date_value):
        _fail(f"{label} date is not YYYY-MM-DD: {date_value!r}")
    reason = item["reason"]
    if not isinstance(reason, str) or not reason.strip():
        _fail(f"{label} reason is empty")
    base_commit = item["base_commit"]
    if not _is_hex(base_commit, COMMIT_RE):
        _fail(f"{label} base_commit is not 40 lowercase hex: {base_commit!r}")
    candidate = item["candidate_sha256"]
    if not _is_hex(candidate, SHA256_RE):
        _fail(f"{label} candidate_sha256 is not 64 lowercase hex: {candidate!r}")
    label_value = item["label"]
    if label_value not in LABELS:
        _fail(f"{label} label is not one of {sorted(LABELS)}: {label_value!r}")
    for key in ("max_bytes", "max_tokens"):
        value = item[key]
        if not _is_int(value) or value <= 0:
            _fail(f"{label} {key} is not a positive integer: {value!r}")
    return {key: item[key] for key in RECORD_KEYS}


def _ordered(records) -> None:
    for previous, current in zip(records, records[1:]):
        if current["date"] < previous["date"]:
            _fail(
                f"records are not ordered by date: {current['decision_id']!r} "
                f"({current['date']}) follows {previous['decision_id']!r} "
                f"({previous['date']})"
            )


def _unique(records) -> None:
    seen = set()
    for record in records:
        if record["decision_id"] in seen:
            _fail(f"duplicate decision_id: {record['decision_id']!r}")
        seen.add(record["decision_id"])


def _records(value) -> list:
    if not isinstance(value, list):
        _fail(f"records must be a list, got {type(value).__name__}")
    records = [_record(item, index) for index, item in enumerate(value)]
    _ordered(records)
    _unique(records)
    return records


def validate(value) -> dict:
    """Return ``value`` normalized as an allowance document, or raise on any problem."""
    if not isinstance(value, dict):
        _fail(f"record file must be a JSON object, got {type(value).__name__}")
    keys = set(value)
    if keys != TOP_LEVEL:
        _fail(_key_message("top-level", keys, TOP_LEVEL))
    if value["format"] != FORMAT:
        _fail(f"format is {value['format']!r}, expected {FORMAT!r}")
    if value["format_version"] != FORMAT_VERSION:
        _fail(f"format_version is {value['format_version']!r}, expected {FORMAT_VERSION!r}")
    return {
        "format": FORMAT,
        "format_version": FORMAT_VERSION,
        "baseline": _baseline(value["baseline"]),
        "records": _records(value["records"]),
    }


def load_document(path) -> dict:
    """Return the validated allowance document at ``path``."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        _fail(f"cannot read record file: {exc}")
    try:
        document = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        _fail(f"record file is not valid UTF-8: {exc}")
    except json.JSONDecodeError as exc:
        _fail(f"record file is not valid JSON: {exc}")
    return validate(document)


def load(path) -> list:
    """Return the validated, date-ordered records in the file at ``path``."""
    return load_document(path)["records"]


def skip() -> list:
    """Return the empty record list; a candidate above baseline then fails."""
    return []


def applicable(records, candidate_sha256):
    """Return the one record naming ``candidate_sha256``, or None when none does."""
    matches = [
        record
        for record in records
        if isinstance(record, dict) and record.get("candidate_sha256") == candidate_sha256
    ]
    if not matches:
        return None
    if len(matches) > 1:
        ids = ", ".join(str(record.get("decision_id")) for record in matches)
        raise AllowanceError(f"allowance: ambiguous records for one candidate digest: {ids}")
    return matches[0]


def within(record, candidate_bytes, candidate_tokens):
    """Return whether both measured values fit their ceilings, plus per-measure problems.

    The ceiling of one measure says nothing about the other, so the problems are
    keyed by measure: a caller gives each arm only its own list and never leaks a
    breach of one measure into the other. Every message names its own measure and
    only that measure. A measure inside its ceiling contributes an empty list.
    """
    problems = {measure: [] for measure in MEASURES}
    for measure, value, ceiling in (
        ("bytes", candidate_bytes, record["max_bytes"]),
        ("tokens", candidate_tokens, record["max_tokens"]),
    ):
        if value > ceiling:
            problems[measure].append(
                f"{measure}: candidate {value} exceeds allowance ceiling {ceiling} "
                f"for decision {record['decision_id']!r}"
            )
    return (not any(problems.values()), problems)
