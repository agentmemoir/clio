#!/usr/bin/env python3
"""Load and validate the ordered record of chosen MCP budget values.

A decision names one preset, the bytes and tokens chosen for it, and the frozen
presets row it is derived from, so a chosen value is traceable to the measurement
that supports it instead of being stated on trust. A record never names its
landing commit: that commit comes from git history, and a record that carries a
commit field is malformed. A chosen value is never a gate input, so nothing here
can loosen or tighten a budget check.

Every error is one line prefixed ``decisions:`` and names the rule it broke.
"""

import argparse
import json
import re
import sys
from pathlib import Path

DECISIONS_PATH = "benchmarks/mcp_token_budget/decisions/frozen-v1.json"
PRESETS_PATH = "benchmarks/mcp_token_budget/presets/frozen-v1.json"

RECORD_FORMAT = "clio_mcp_budget_decisions"
RECORD_FORMAT_VERSION = 1
RECORD_ID = "budget-decisions-v1"
DECISION_LABEL = "Decision"
DERIVED_JUDGEMENT = "derived"
MEASURED_LABEL = "Measured"

TOP_LEVEL = frozenset({"format", "format_version", "record_id", "frozen", "policy", "records"})
POLICY_KEYS = frozenset({"gate_input", "rule"})
RECORD_KEYS = (
    "decision_id", "date", "label", "judgement", "preset",
    "bytes", "tokens", "derived_from", "reason", "base_commit",
)
RECORD_KEY_SET = frozenset(RECORD_KEYS)
DERIVED_KEYS = ("record", "record_id", "row", "catalog_hash", "input_sha256")
DERIVED_KEY_SET = frozenset(DERIVED_KEYS)
COMMIT_KEYS = ("commit", "landing_commit", "containing_commit")

#: Each cited measure and where the record states it: None at the top, else a block.
CITED_MEASURES = (
    ("bytes", None), ("tokens", None),
    ("catalog_hash", "derived_from"), ("input_sha256", "derived_from"),
)

COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
_HERE = Path(__file__).resolve().parent


class DecisionError(ValueError):
    """Raised when a decision record cannot be trusted."""


def _fail(message):
    raise DecisionError(f"decisions: {message}")


def _is_int(value) -> bool:
    """Return true for a JSON integer and false for a boolean."""
    return isinstance(value, int) and not isinstance(value, bool)


def _object(value, where, allowed) -> dict:
    """Return ``value`` as an object with exactly the ``allowed`` keys."""
    if not isinstance(value, dict):
        _fail(f"{where} must be an object, got {type(value).__name__}")
    keys = set(value)
    if keys != allowed:
        _fail(f"{where} keys must be exactly {sorted(allowed)}: missing "
              f"{sorted(allowed - keys)}, unexpected {sorted(keys - allowed)}")
    return value


def _nonempty(value, where) -> str:
    """Return ``value`` as a non-empty string, or refuse it."""
    if not isinstance(value, str) or not value.strip():
        _fail(f"{where} must be a nonempty string, got {value!r}")
    return value


def _expect(value, where, expected) -> None:
    """Refuse ``value`` unless it is exactly ``expected``."""
    if value != expected or type(value) is not type(expected):
        _fail(f"{where} is {value!r}, expected {expected!r}")


def _no_commit(value, where) -> None:
    """Refuse any commit-naming key at any depth of one record."""
    if not isinstance(value, (dict, list)):
        return
    pairs = value.items() if isinstance(value, dict) else [(None, one) for one in value]
    for key, item in pairs:
        if key in COMMIT_KEYS:
            _fail(f"{where} carries a {key!r} field: the landing commit comes "
                  "from git history, never from a record")
        _no_commit(item, where)


def _cited_row(presets, preset, label) -> dict:
    """Return the presets row ``preset`` names, or refuse a row it does not hold once."""
    rows = presets.get("rows") if isinstance(presets, dict) else None
    if not isinstance(rows, list):
        _fail(f"presets record has no rows list, got {type(rows).__name__}")
    matches = [row for row in rows if isinstance(row, dict) and row.get("preset") == preset]
    if len(matches) != 1:
        _fail(f"{label} cites preset {preset!r}, which the presets record at "
              f"{PRESETS_PATH} holds {len(matches)} times")
    return matches[0]


def _record(item, index, presets) -> dict:
    """Return one decision record, or refuse a value its cited row does not support."""
    named = isinstance(item, dict) and isinstance(item.get("decision_id"), str)
    label = f"record {item['decision_id']!r}" if named else f"record {index}"
    _no_commit(item, label)
    item = _object(item, label, RECORD_KEY_SET)
    _nonempty(item["decision_id"], f"{label} decision_id")
    if not isinstance(item["date"], str) or not DATE_RE.match(item["date"]):
        _fail(f"{label} date is not YYYY-MM-DD: {item['date']!r}")
    _expect(item["label"], f"{label} label", DECISION_LABEL)
    _expect(item["judgement"], f"{label} judgement", DERIVED_JUDGEMENT)
    preset = _nonempty(item["preset"], f"{label} preset")
    for key in ("bytes", "tokens"):
        if not _is_int(item[key]) or item[key] <= 0:
            _fail(f"{label} {key} is not a positive integer: {item[key]!r}")
    cited = _object(item["derived_from"], f"{label} derived_from", DERIVED_KEY_SET)
    _expect(cited["record"], f"{label} derived_from.record", PRESETS_PATH)
    _expect(cited["row"], f"{label} derived_from.row", preset)
    _expect(cited["record_id"], f"{label} derived_from.record_id", presets.get("record_id"))
    row = _cited_row(presets, preset, label)
    _expect(row.get("label"), f"preset {preset!r} cited row label", MEASURED_LABEL)
    for field, group in CITED_MEASURES:
        stated = cited[field] if group else item[field]
        if stated != row.get(field):
            _fail(f"{label} {field} is {stated!r}, but the presets record's "
                  f"{preset!r} row states {row.get(field)!r}")
    _nonempty(item["reason"], f"{label} reason")
    commit = item["base_commit"]
    if not isinstance(commit, str) or not COMMIT_RE.match(commit):
        _fail(f"{label} base_commit is not 40 lowercase hex: {commit!r}")
    return {key: item[key] for key in RECORD_KEYS}


def validate(document, presets) -> list:
    """Return ``document``'s date-ordered decision records, or raise on any problem."""
    document = _object(document, "top-level", TOP_LEVEL)
    _expect(document["format"], "format", RECORD_FORMAT)
    _expect(document["format_version"], "format_version", RECORD_FORMAT_VERSION)
    _expect(document["record_id"], "record_id", RECORD_ID)
    _expect(document["frozen"], "frozen", False)
    policy = _object(document["policy"], "policy", POLICY_KEYS)
    _expect(policy["gate_input"], "policy.gate_input", False)
    _nonempty(policy["rule"], "policy.rule")
    if not isinstance(document["records"], list):
        _fail(f"records must be a list, got {type(document['records']).__name__}")
    records = [_record(one, index, presets)
               for index, one in enumerate(document["records"])]
    for previous, current in zip(records, records[1:]):
        if current["date"] < previous["date"]:
            _fail(f"records are not ordered by date: {current['decision_id']!r} "
                  f"({current['date']}) follows {previous['decision_id']!r} "
                  f"({previous['date']})")
    seen = set()
    for record in records:
        if record["decision_id"] in seen:
            _fail(f"duplicate decision_id: {record['decision_id']!r}")
        seen.add(record["decision_id"])
    return records


def _read(path, where) -> dict:
    """Return the parsed JSON document at ``path``, or refuse it."""
    try:
        return json.loads(Path(path).read_bytes().decode("utf-8"))
    except OSError as exc:
        _fail(f"cannot read {where}: {exc}")
    except UnicodeDecodeError as exc:
        _fail(f"{where} is not valid UTF-8: {exc}")
    except json.JSONDecodeError as exc:
        _fail(f"{where} is not valid JSON: {exc}")


def load(path, presets_path=PRESETS_PATH) -> list:
    """Return the validated, date-ordered decisions in the record at ``path``."""
    presets = _read(presets_path, "presets file")
    return validate(_read(path, "decision file"), presets)


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this module's stderr format."""

    def error(self, message):
        sys.stderr.write(f"decisions: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Validate the committed record and print one summary line; return the exit code."""
    parser = _Parser(prog="budget_decisions.py",
                     description="Validate the MCP budget decision record.")
    parser.add_argument("--check", action="store_true",
                        help="validate the committed record and print a summary line")
    parser.parse_args(argv)
    try:
        records = load(_HERE / "decisions" / "frozen-v1.json",
                       _HERE / "presets" / "frozen-v1.json")
    except DecisionError as exc:
        sys.stderr.write(f"{exc}\n")
        return 2
    noun = "record" if len(records) == 1 else "records"
    sys.stdout.write(f"decisions: {len(records)} {noun}, ok\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
