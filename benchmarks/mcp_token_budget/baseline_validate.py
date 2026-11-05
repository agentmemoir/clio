#!/usr/bin/env python3
"""Validate one frozen MCP tool-surface baseline record.

The validator reads a record file and returns a list of error strings. Every
string starts with the acceptance criterion it checks, so a failure names its
rule. The module is stdlib only, so the hermetic job can run it without the
tokenizer package.

``main()`` prints each error and exits 2 when the list is non-empty. It exits 0
when the list is empty and prints nothing.
"""

import argparse
import hashlib
import json
import re
import sys
from datetime import date
from pathlib import Path

RECORD_FORMAT = "clio_mcp_frozen_baseline"
FORMAT_VERSION = 1
RECORD_ID = "mcp-tool-surface-2026-11-05"

LABELS = frozenset(
    {"Measured", "Vendor-measured", "External", "Inferred", "Decision", "Unverified"}
)
UNITS = frozenset({"bytes", "tokens"})
DELIVERIES = frozenset({"eager", "on_demand"})

TOKENIZER_ENCODING = "cl100k_base"
TOKENIZER_IMPLEMENTATION = "tiktoken"
TOKENIZER_VERSION = "0.14.0"
ASSET_SHA256 = "223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7"
RETIRED_SKILL_SHA256 = (
    "51ec302b11becd64c388c25b88495a95bcf7705ba06142519a275c0b76acba80"
)
RETIRED_ROWS = ("retired_skill_entry_point/bytes", "retired_skill_entry_point/tokens")

ALLOWED_TOP_LEVEL = frozenset(
    {
        "format",
        "format_version",
        "record_id",
        "frozen",
        "frozen_on",
        "supersedes",
        "policy",
        "tokenizer",
        "pack",
        "clients",
        "rows",
        "per_tool_ranking",
        "unobserved",
        "receipts",
    }
)

DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def _is_int(value) -> bool:
    """Return true for a JSON integer and false for a boolean."""
    return isinstance(value, int) and not isinstance(value, bool)


def _is_date(value) -> bool:
    """Return true when ``value`` is a real YYYY-MM-DD date string."""
    if not isinstance(value, str) or not DATE_RE.match(value):
        return False
    try:
        date.fromisoformat(value)
    except ValueError:
        return False
    return True


def _objects(value):
    """Yield every JSON object nested anywhere under ``value``."""
    if isinstance(value, dict):
        yield value
        for item in value.values():
            yield from _objects(item)
    elif isinstance(value, list):
        for item in value:
            yield from _objects(item)


def _strings(value):
    """Yield every string key and value nested anywhere under ``value``."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from _strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from _strings(item)


def _rows(record):
    rows = record.get("rows")
    return [row for row in rows if isinstance(row, dict)] if isinstance(rows, list) else []


def _client_rows(record, client, turn, unit):
    return [row for row in _rows(record) if row.get("row") == f"client/{client}/turn/{turn}/{unit}"]


def _canonical_tools(tools):
    """Serialize one turn's tools array the way the replay module measures it."""
    return json.dumps(tools, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _fixture_turns(path, name):
    """Return ``({turn: tools}, None)`` for one fixture, or ``(None, error)``."""
    try:
        turns = json.loads(path.read_text(encoding="utf-8"))["turns"]
        return {item["turn"]: item["payload"]["tools"] for item in turns}, None
    except (OSError, ValueError, KeyError, TypeError) as exc:
        return None, f"AC3: client {name!r} fixture cannot be read: {exc}"


def _expected_client_ids(record):
    """Return the client row ids the declared clients and turns imply."""
    clients = record.get("clients")
    if not isinstance(clients, list):
        return set()
    return {
        f"client/{client['client']}/turn/{turn}/{unit}"
        for client in clients
        if isinstance(client, dict)
        and isinstance(client.get("client"), str)
        and isinstance(client.get("turns"), list)
        for turn in client["turns"]
        if _is_int(turn) and turn > 0
        for unit in ("bytes", "tokens")
    }


def check_record(record, base_dir) -> list:
    """Return every error in one record object, in check order."""
    errors = []

    keys = set(record)
    for missing in sorted(ALLOWED_TOP_LEVEL - keys):
        errors.append(f"AC6: missing top-level key: {missing}")
    for extra in sorted(keys - ALLOWED_TOP_LEVEL):
        errors.append(f"AC6: unexpected top-level key: {extra}")

    if record.get("format") != RECORD_FORMAT:
        errors.append(f"AC6: record format is {record.get('format')!r}, expected {RECORD_FORMAT!r}")
    if record.get("format_version") != FORMAT_VERSION:
        errors.append(
            f"AC6: format_version is {record.get('format_version')!r}, "
            f"expected {FORMAT_VERSION!r}"
        )
    if record.get("record_id") != RECORD_ID:
        errors.append(
            f"AC6: record_id is {record.get('record_id')!r}, expected {RECORD_ID!r}"
        )

    if record.get("frozen") is not True:
        errors.append("AC6: frozen must be true")
    supersedes = record.get("supersedes")
    if supersedes is not None and supersedes == record.get("record_id"):
        errors.append("AC6: supersedes must be null or a different record id")

    policy = record.get("policy")
    if not isinstance(policy, dict):
        errors.append("AC6: policy must be an object")
    else:
        if policy.get("append_allowed") is not False:
            errors.append("AC6: policy.append_allowed must be false")
        later = policy.get("later_measurements")
        if not isinstance(later, str) or not later.strip():
            errors.append("AC6: policy.later_measurements must be non-empty")

    if not _is_date(record.get("frozen_on")):
        errors.append(f"AC5: frozen_on is not a real date: {record.get('frozen_on')!r}")
    for row in _rows(record):
        if not _is_date(row.get("date")):
            errors.append(f"AC5: row {row.get('row')!r} date is not a real date")
    ranking = record.get("per_tool_ranking")
    items = ranking.get("items") if isinstance(ranking, dict) else None
    if isinstance(items, list):
        for item in items:
            if not isinstance(item, dict) or not _is_date(item.get("date")):
                errors.append(f"AC5: ranking item {item!r} date is not a real date")

    tokenizer = record.get("tokenizer")
    if not isinstance(tokenizer, dict):
        errors.append("AC8: tokenizer must be an object")
    else:
        if tokenizer.get("encoding") != TOKENIZER_ENCODING:
            errors.append(
                f"AC8: tokenizer encoding is {tokenizer.get('encoding')!r}, "
                f"expected {TOKENIZER_ENCODING!r}"
            )
        if tokenizer.get("asset_sha256") != ASSET_SHA256:
            errors.append("AC8: tokenizer asset_sha256 does not match the pinned asset")

    clients = record.get("clients")
    if not isinstance(clients, list) or not clients:
        errors.append("AC1: clients must be a non-empty list")
        clients = []
    for client in clients:
        if not isinstance(client, dict):
            errors.append(f"AC1: client entry must be an object: {client!r}")
            continue
        name = client.get("client")
        if not isinstance(name, str) or not name.strip():
            errors.append(f"AC1: client name is empty: {client!r}")
        version = client.get("client_version")
        if not isinstance(version, str) or not version.strip():
            errors.append(f"AC1: client {name!r} has no client_version")
        if client.get("delivery") not in DELIVERIES:
            errors.append(
                f"AC1: client {name!r} delivery is {client.get('delivery')!r}, "
                f"expected one of {sorted(DELIVERIES)}"
            )
        fixture = client.get("fixture")
        if isinstance(name, str) and name.startswith("synthetic"):
            if not isinstance(fixture, dict):
                errors.append(f"AC2: client {name!r} has no fixture object")
            else:
                if not isinstance(fixture.get("name"), str) or not fixture["name"].strip():
                    errors.append(f"AC2: client {name!r} fixture has no name")
                if not _is_int(fixture.get("version")) or fixture["version"] < 1:
                    errors.append(f"AC2: client {name!r} fixture has no version")
                if not isinstance(fixture.get("sha256"), str) or not SHA256_RE.match(
                    fixture["sha256"]
                ):
                    errors.append(f"AC2: client {name!r} fixture sha256 is not 64 hex")
        turns = client.get("turns")
        if not isinstance(turns, list) or not turns:
            errors.append(f"AC3: client {name!r} has no turns")
            continue
        seen = set()
        fixture_name = fixture.get("name") if isinstance(fixture, dict) else None
        by_turn = None
        if isinstance(fixture_name, str):
            by_turn, fixture_error = _fixture_turns(
                Path(base_dir) / "clients" / fixture_name, name
            )
            if fixture_error:
                errors.append(fixture_error)
        for turn in turns:
            if not _is_int(turn) or turn < 1:
                errors.append(f"AC3: client {name!r} turn is not a positive integer: {turn!r}")
                continue
            if turn in seen:
                errors.append(f"AC3: client {name!r} repeats turn {turn}")
                continue
            seen.add(turn)
            for unit in ("bytes", "tokens"):
                rows = _client_rows(record, name, turn, unit)
                if len(rows) != 1:
                    errors.append(
                        f"AC3: expected one {unit} row for {name} turn {turn}, found {len(rows)}"
                    )
                elif not isinstance(rows[0].get("label"), str):
                    errors.append(f"AC3: row {rows[0].get('row')!r} has no label")
            if by_turn is None:
                continue
            if turn not in by_turn:
                errors.append(f"AC3: client {name!r} turn {turn} is not in its fixture")
                continue
            canonical = _canonical_tools(by_turn[turn])
            digest = hashlib.sha256(canonical).hexdigest()
            bytes_rows = _client_rows(record, name, turn, "bytes")
            tokens_rows = _client_rows(record, name, turn, "tokens")
            if len(bytes_rows) == 1:
                if bytes_rows[0].get("value") != len(canonical):
                    errors.append(
                        f"AC3: client {name!r} turn {turn} bytes value does not match the fixture"
                    )
                if bytes_rows[0].get("input_sha256") != digest:
                    errors.append(
                        f"AC3: client {name!r} turn {turn} bytes input_sha256 does not match the fixture"
                    )
            if len(tokens_rows) == 1 and tokens_rows[0].get("input_sha256") != digest:
                errors.append(
                    f"AC3: client {name!r} turn {turn} tokens input_sha256 does not match the fixture"
                )

    expected_ids = _expected_client_ids(record) | set(RETIRED_ROWS)
    seen_ids = []
    for row in _rows(record):
        row_id = row.get("row")
        if not isinstance(row_id, str) or not row_id:
            errors.append(f"AC3: row has no id: {row!r}")
            continue
        if row_id in seen_ids:
            errors.append(f"AC3: duplicate row id: {row_id}")
        seen_ids.append(row_id)
        if row_id not in expected_ids:
            errors.append(f"AC3: unexpected row id: {row_id}")
        value = row.get("value")
        if not _is_int(value) or value < 0:
            errors.append(f"AC3: row {row_id} value is not an integer >= 0")
        if row.get("unit") not in UNITS:
            errors.append(f"AC3: row {row_id} unit is not bytes or tokens")

    for obj in _objects(record):
        if "value" not in obj:
            continue
        if not _is_date(obj.get("date")):
            errors.append(f"AC5: object with a value has no real date: {obj.get('row')!r}")
        if obj.get("label") not in LABELS:
            errors.append(f"AC5: object with a value has no valid label: {obj.get('row')!r}")
        if obj.get("label") != "Measured":
            continue
        if (
            obj.get("estimated") is True
            or obj.get("fallback") is True
            or obj.get("unit") not in UNITS
        ):
            errors.append(
                f"AC8: row {obj.get('row')!r} is labelled Measured but is an estimate, "
                f"a fallback, or outside the pinned units"
            )
        row_id = obj.get("row")
        if isinstance(row_id, str) and row_id.startswith("client/"):
            digest = obj.get("input_sha256")
            if not isinstance(digest, str) or not SHA256_RE.match(digest):
                errors.append(f"AC8: row {row_id} is labelled Measured but has no input_sha256")
        elif row_id in RETIRED_ROWS:
            receipt = obj.get("receipt_id")
            receipts = record.get("receipts")
            if not isinstance(receipt, str) or not receipt.strip():
                errors.append(f"AC8: row {row_id} is labelled Measured but has no receipt_id")
            elif not isinstance(receipts, dict) or not isinstance(receipts.get(receipt), dict):
                errors.append(f"AC8: row {row_id} receipt_id does not resolve")

    for row_id in RETIRED_ROWS:
        count = len([row for row in _rows(record) if row.get("row") == row_id])
        if count != 1:
            errors.append(f"AC7: expected exactly one {row_id} row, found {count}")
    for row in _rows(record):
        if row.get("row") not in RETIRED_ROWS:
            continue
        if row.get("content_sha256") != RETIRED_SKILL_SHA256:
            errors.append(f"AC7: {row.get('row')} content_sha256 does not match the pinned digest")
        if not _is_int(row.get("value")) or row["value"] <= 0:
            errors.append(f"AC7: {row.get('row')} value must be greater than zero")

    if not isinstance(ranking, dict):
        errors.append("AC4: per_tool_ranking must be an object")
    else:
        if ranking.get("sorted_by") != "tool_tokens":
            errors.append("AC4: per_tool_ranking.sorted_by must be tool_tokens")
        if not isinstance(items, list):
            errors.append("AC4: per_tool_ranking.items must be a list")
        else:
            for item in items:
                if not isinstance(item, dict) or "tool_tokens" not in item:
                    errors.append(f"AC4: ranking item has no tool_tokens: {item!r}")
            if len(items) != 10:
                errors.append(f"AC4: ranking must hold 10 items, found {len(items)}")
            ranks = [item.get("rank") if isinstance(item, dict) else None for item in items]
            if ranks != list(range(1, 11)):
                errors.append(f"AC4: ranking ranks must be 1..10 in order, found {ranks!r}")
            def _order(item):
                tokens = item.get("tool_tokens")
                tokens = tokens if _is_int(tokens) else 0
                return (-tokens, str(item.get("tool", "")))
            ordered = sorted(
                [item for item in items if isinstance(item, dict)], key=_order
            )
            if [item.get("tool") for item in items if isinstance(item, dict)] != [
                item.get("tool") for item in ordered
            ]:
                errors.append(
                    "AC4: ranking is not ordered by tool_tokens with tool name "
                    "as the tie-break"
                )
            for item in items:
                if not isinstance(item, dict):
                    continue
                if not isinstance(item.get("tool"), str) or not item["tool"].strip():
                    errors.append(f"AC4: ranking item has no tool name: {item!r}")
                for key in ("tool_bytes", "tool_tokens"):
                    if not _is_int(item.get(key)) or item[key] < 0:
                        errors.append(f"AC4: ranking item {item.get('tool')!r} has no valid {key}")
                if item.get("label") not in LABELS:
                    errors.append(f"AC4: ranking item {item.get('tool')!r} has no valid label")

    unobserved = record.get("unobserved")
    if not isinstance(unobserved, list):
        errors.append("AC5: unobserved must be a list")
    else:
        for entry in unobserved:
            if not isinstance(entry, dict):
                errors.append(f"AC5: unobserved entry must be an object: {entry!r}")
                continue
            if not _is_date(entry.get("date")):
                errors.append("AC5: unobserved entry has no real date")
            if entry.get("label") != "Unverified":
                errors.append("AC5: unobserved entry label must be Unverified")
            reason = entry.get("reason")
            if not isinstance(reason, str) or not reason.strip():
                errors.append("AC5: unobserved entry needs a non-empty reason")
            if "value" in entry:
                errors.append("AC5: unobserved entry must not carry a value")

    for text in _strings(record):
        if text.startswith("/"):
            errors.append("path boundary: record holds an absolute path")
            break

    return errors


def validate(path) -> list:
    """Return every error in the record at ``path``; an empty list means it holds."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        return [f"AC6: cannot read record: {exc}"]
    try:
        record = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        return [f"AC6: record is not valid UTF-8: {exc}"]
    except json.JSONDecodeError as exc:
        return [f"AC6: record is not valid JSON: {exc}"]
    if not isinstance(record, dict):
        return [f"AC6: record must be a JSON object, got {type(record).__name__}"]
    return check_record(record, Path(path).resolve().parent)


def main(argv=None) -> int:
    """Validate one record file; return 0 when it holds and 2 when it does not."""
    parser = argparse.ArgumentParser(
        prog="baseline_validate.py",
        description="Validate one frozen MCP tool-surface baseline record.",
    )
    parser.add_argument("record", metavar="PATH", help="record file to validate")
    args = parser.parse_args(argv)

    errors = validate(args.record)
    for error in errors:
        sys.stderr.write(f"mcp-baseline-validate: {error}\n")
    return 2 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
