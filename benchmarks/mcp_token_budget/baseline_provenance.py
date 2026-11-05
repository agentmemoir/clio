#!/usr/bin/env python3
"""Prove a frozen baseline record's numbers and identities against committed bytes.

The module loads each replay fixture named by the record and recomputes the
canonical bytes and digest of every turn. It checks the rows against those
values, checks each fixture digest, checks the tokenizer identity, and checks
the pack block. It has no CLI and does not import the structural validator.

Every error string starts with the acceptance criterion it checks, so a caller
can print the list without adding context.
"""

import hashlib
import json
from pathlib import Path

PACK_FORMAT = "am_tool_schema_pack"
PACK_FORMAT_VERSION = 1
TOKENIZER_ENCODING = "cl100k_base"
TOKENIZER_IMPLEMENTATION = "tiktoken"
TOKENIZER_VERSION = "0.14.0"


def canonical_tools(tools):
    """Serialize one tools array with sorted keys, no spaces, and UTF-8."""
    return json.dumps(tools, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _client_rows(record, client, turn, unit):
    row_id = f"client/{client}/turn/{turn}/{unit}"
    rows = record.get("rows")
    rows = rows if isinstance(rows, list) else []
    return [row for row in rows if isinstance(row, dict) and row.get("row") == row_id]


def _fixture(path, client):
    """Return ``({turn: tools}, raw, None)`` or ``(None, raw or None, error)``."""
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return None, None, f"AC2: client {client!r} fixture cannot be read: {exc}"
    try:
        turns = json.loads(raw.decode("utf-8"))["turns"]
        return {item["turn"]: item["payload"]["tools"] for item in turns}, raw, None
    except (UnicodeDecodeError, ValueError, KeyError, TypeError) as exc:
        return None, raw, f"AC3: client {client!r} fixture is not a valid replay: {exc}"


def _check_turn(record, client, turn, tools, errors):
    """Check one turn's bytes value and both digests against its tools array."""
    canonical = canonical_tools(tools)
    digest = hashlib.sha256(canonical).hexdigest()
    bytes_rows = _client_rows(record, client, turn, "bytes")
    tokens_rows = _client_rows(record, client, turn, "tokens")
    if len(bytes_rows) == 1:
        if bytes_rows[0].get("value") != len(canonical):
            errors.append(
                f"AC3: client {client!r} turn {turn} bytes value does not match the fixture"
            )
        if bytes_rows[0].get("input_sha256") != digest:
            errors.append(
                f"AC3: client {client!r} turn {turn} bytes input_sha256 does not match the fixture"
            )
    if len(tokens_rows) == 1 and tokens_rows[0].get("input_sha256") != digest:
        errors.append(
            f"AC3: client {client!r} turn {turn} tokens input_sha256 does not match the fixture"
        )


def _check_pack(record, clients, fixtures, errors):
    """Check the pack block against the eager fixture and the pack constants."""
    pack = record.get("pack")
    if not isinstance(pack, dict):
        errors.append("AC3: pack must be an object")
        return
    if pack.get("format") != PACK_FORMAT:
        errors.append(f"AC3: pack format is {pack.get('format')!r}, expected {PACK_FORMAT!r}")
    if pack.get("format_version") != PACK_FORMAT_VERSION:
        errors.append(
            f"AC3: pack format_version is {pack.get('format_version')!r}, "
            f"expected {PACK_FORMAT_VERSION!r}"
        )
    eager = next(
        (item for item in clients if isinstance(item, dict) and item.get("delivery") == "eager"),
        None,
    )
    name = eager.get("client") if isinstance(eager, dict) else None
    by_turn = fixtures.get(name)
    tools = by_turn.get(1) if isinstance(by_turn, dict) else None
    if not isinstance(tools, list):
        errors.append("AC3: eager fixture turn 1 tools are missing")
        return
    if pack.get("tools_count") != len(tools):
        errors.append("AC3: pack tools_count does not match the eager fixture")
    whole = {
        "format": pack.get("format"),
        "format_version": pack.get("format_version"),
        "catalog_hash": pack.get("catalog_hash"),
        "mcp_protocol_revision": pack.get("mcp_protocol_revision"),
        "tools": tools,
    }
    canonical = canonical_tools(whole)
    if pack.get("canonical_bytes") != len(canonical):
        errors.append("AC3: pack canonical_bytes does not match the recomputed pack")
    if pack.get("canonical_sha256") != hashlib.sha256(canonical).hexdigest():
        errors.append("AC3: pack canonical_sha256 does not match the recomputed pack")


def _check_recomputable(record, errors):
    """Check every Measured row's recomputable flag and its receipt."""
    receipts = record.get("receipts")
    rows = record.get("rows")
    rows = rows if isinstance(rows, list) else []
    for row in rows:
        if not isinstance(row, dict) or row.get("label") != "Measured":
            continue
        row_id = row.get("row")
        flag = row.get("recomputable")
        if not isinstance(flag, bool):
            errors.append(f"AC8: row {row_id!r} recomputable is not a boolean")
            continue
        if flag:
            if row.get("unit") == "tokens" and not row.get("input_sha256"):
                errors.append(f"AC8: row {row_id!r} input_sha256 is missing")
            continue
        receipt_id = row.get("receipt_id")
        receipt = receipts.get(receipt_id) if isinstance(receipts, dict) else None
        if not isinstance(receipt, dict):
            errors.append(f"AC8: row {row_id!r} receipt_id does not resolve")
            continue
        if receipt.get("recomputable_from_public_inputs") is not False:
            errors.append(
                f"AC8: row {row_id!r} receipt recomputable_from_public_inputs is not false"
            )
        for key, expected in (
            ("encoding", TOKENIZER_ENCODING),
            ("implementation", TOKENIZER_IMPLEMENTATION),
            ("version", TOKENIZER_VERSION),
        ):
            if receipt.get(key) != expected:
                errors.append(
                    f"AC8: row {row_id!r} receipt {key} does not match the pinned tokenizer"
                )


def check(record, base_dir):
    """Return every provenance error in ``record`` against fixtures under ``base_dir``."""
    errors = []
    clients = record.get("clients")
    if not isinstance(clients, list):
        return errors
    fixtures = {}
    for client in clients:
        if not isinstance(client, dict) or not isinstance(client.get("client"), str):
            continue
        name = client["client"]
        fixture = client.get("fixture")
        if not isinstance(fixture, dict) or not isinstance(fixture.get("name"), str):
            continue
        by_turn, raw, error = _fixture(Path(base_dir) / "clients" / fixture["name"], name)
        if error:
            errors.append(error)
            continue
        if raw is not None and hashlib.sha256(raw).hexdigest() != fixture.get("sha256"):
            errors.append(f"AC2: client {name!r} fixture sha256 does not match the file")
        if by_turn is None:
            continue
        fixtures[name] = by_turn
        turns = client.get("turns")
        if not isinstance(turns, list):
            continue
        for item in turns:
            turn = item.get("turn") if isinstance(item, dict) else None
            if not isinstance(turn, int) or isinstance(turn, bool) or turn < 1:
                continue
            if turn not in by_turn:
                errors.append(f"AC3: client {name!r} turn {turn} is not in its fixture")
                continue
            _check_turn(record, name, turn, by_turn[turn], errors)
    tokenizer = record.get("tokenizer")
    if isinstance(tokenizer, dict):
        if tokenizer.get("implementation") != TOKENIZER_IMPLEMENTATION:
            errors.append(
                f"AC8: tokenizer implementation is {tokenizer.get('implementation')!r}, "
                f"expected {TOKENIZER_IMPLEMENTATION!r}"
            )
        if tokenizer.get("version") != TOKENIZER_VERSION:
            errors.append(
                f"AC8: tokenizer version is {tokenizer.get('version')!r}, "
                f"expected {TOKENIZER_VERSION!r}"
            )
    _check_recomputable(record, errors)
    _check_pack(record, clients, fixtures, errors)
    return errors
