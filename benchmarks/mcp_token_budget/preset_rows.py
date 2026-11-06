#!/usr/bin/env python3
"""Record the measured byte and token size of each MCP tool preset.

Each preset is exported by ``clio mcp schema-export --tools <preset> --out FILE``
into a temporary directory, validated as an ``am_tool_schema_pack`` carrying a
non-empty ``tools`` array, and only that array is measured: re-serialized with
sorted keys, ``(",", ":")`` separators, and UTF-8, its bytes give the byte
length, the cl100k_base token count, and the sha256 that ties a row back to the
surface it describes. The pack envelope is excluded, so a preset row and the
client baseline rows it names are the same quantity. The exact export of every
preset is kept as a fixture beside the record, written from the bytes the binary
produced and never edited here.

Tokens come from the repo-pinned offline tokenizer, so this script performs no
network I/O and never substitutes an estimate. Every failure exits 2 with
``mcp-preset-rows: <message>`` on stderr and writes no record.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from offline_tokenizer import BudgetError, load

PACK_FORMAT = "am_tool_schema_pack"
RECORD_FORMAT = "am_mcp_preset_surface"
RECORD_FORMAT_VERSION = 1
RECORD_ID = "preset-surface-v1"
PRESETS = ("minimal", "coding", "ops", "all")
FULL_PRESET = "all"
BASELINE_ROW = "client/synthetic-eager-client/turn/1"
BASELINE_RECORD_PATH = "benchmarks/mcp_token_budget/baseline/frozen-v1.json"
DELIVERY = "eager"
DELIVERY_NOTE = (
    "Eager delivery means every tool definition with its input schema is delivered "
    "at turn 1, and this build has no discovery mode, so this is the discovery-off "
    "surface."
)
NORMALIZATION = (
    "canonical bytes of the pack's tools array only; the pack envelope is excluded"
)

_REPO_ROOT = Path(__file__).resolve().parents[2]
_BASELINE_FILE = _REPO_ROOT / BASELINE_RECORD_PATH


def canonical_bytes(value) -> bytes:
    """Serialize ``value`` to the canonical bytes measured for one tools array."""
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def _display(path) -> str:
    """Return ``path`` relative to the repository root, or absolute outside it."""
    resolved = Path(path).resolve()
    try:
        return resolved.relative_to(_REPO_ROOT).as_posix()
    except ValueError:
        return str(resolved)


def _binary(explicit):
    """Return the CLI binary to measure with, or raise BudgetError."""
    if explicit:
        candidates = [Path(explicit)]
    else:
        configured = os.environ.get("CLIO_BIN")
        candidates = [Path(configured)] if configured else []
        candidates.append(_REPO_ROOT / "target" / "release" / "clio")
        candidates.append(_REPO_ROOT / "target" / "debug" / "clio")
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    raise BudgetError(
        "no usable clio binary: pass --bin, set CLIO_BIN, or build target/release/clio"
    )


def _export(binary, preset, path):
    """Run ``mcp schema-export --tools <preset>`` and write the pack to ``path``."""
    command = [
        str(binary), "mcp", "schema-export", "--tools", preset, "--out", str(path)
    ]
    try:
        completed = subprocess.run(command, capture_output=True)
    except OSError as exc:
        raise BudgetError(f"cannot run {binary}: {exc}") from None
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", "replace").strip() or "no stderr"
        raise BudgetError(f"export failed for preset {preset!r}: {detail}")


def _parse_pack(raw, preset):
    """Decode one preset pack and require its format and a non-empty tools array."""
    try:
        pack = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise BudgetError(f"preset {preset!r} pack is not valid JSON: {exc}") from None
    if not isinstance(pack, dict) or pack.get("format") != PACK_FORMAT:
        raise BudgetError(f"preset {preset!r} pack is not a {PACK_FORMAT} object")
    tools = pack.get("tools")
    if not isinstance(tools, list) or not tools:
        raise BudgetError(f"preset {preset!r} pack has no tools")
    return pack


def _row(preset, tools, tokenizer, date):
    """Build one measured preset row from a tools array."""
    canonical = canonical_bytes(tools)
    return {
        "preset": preset,
        "baseline_row": BASELINE_ROW,
        "bytes": len(canonical),
        "tokens": tokenizer.count(canonical.decode("utf-8")),
        "label": "Measured",
        "date": date,
        "input_sha256": hashlib.sha256(canonical).hexdigest(),
    }


def measure(binary, fixtures, tokenizer, date):
    """Export every preset, keep each pack as a fixture, and return ``(catalog, rows)``.

    The recorded catalog hash is the one the ``all`` pack carries, because that
    preset is the whole discovery-off surface the record describes.
    """
    fixtures = Path(fixtures)
    rows = []
    catalog = None
    with tempfile.TemporaryDirectory(prefix="clio-preset-rows-") as staging:
        try:
            fixtures.mkdir(parents=True, exist_ok=True)
            for preset in PRESETS:
                staged = Path(staging) / f"{preset}.json"
                _export(binary, preset, staged)
                raw = staged.read_bytes()
                pack = _parse_pack(raw, preset)
                if preset == FULL_PRESET:
                    catalog = pack.get("catalog_hash")
                rows.append(_row(preset, pack["tools"], tokenizer, date))
                (fixtures / f"{preset}.json").write_bytes(raw)
        except OSError as exc:
            raise BudgetError(f"cannot keep preset fixture: {exc}") from None
    return catalog, rows


def _render(binary, out_path, catalog_hash, rows, tokenizer) -> str:
    """Return the complete record document as one string."""
    try:
        baseline_sha = hashlib.sha256(_BASELINE_FILE.read_bytes()).hexdigest()
    except OSError as exc:
        raise BudgetError(f"cannot read baseline record: {exc}") from None
    document = {
        "format": RECORD_FORMAT,
        "format_version": RECORD_FORMAT_VERSION,
        "record_id": RECORD_ID,
        "frozen": True,
        "policy": {"later_measurements": "separate record file"},
        "baseline_record": {"path": BASELINE_RECORD_PATH, "sha256": baseline_sha},
        "baseline_row": BASELINE_ROW,
        "delivery": DELIVERY,
        "delivery_note": DELIVERY_NOTE,
        "measurement": {
            "command": (
                f"python3 benchmarks/mcp_token_budget/preset_rows.py "
                f"--bin {_display(binary)} --out {_display(out_path)}"
            ),
            "normalization": NORMALIZATION,
            "encoder": tokenizer.encoding,
            "tokenizer": f"tiktoken=={tokenizer.provenance()['version']}",
            "catalog_hash": catalog_hash,
        },
        "rows": rows,
        "unobserved": [],
    }
    return (
        json.dumps(document, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    )


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this tool's stderr format."""

    def error(self, message):
        sys.stderr.write(f"mcp-preset-rows: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Measure every preset, write the record and its fixtures; return the exit code."""
    parser = _Parser(
        prog="preset_rows.py",
        description="Record the measured byte and token size of each MCP tool preset.",
    )
    parser.add_argument(
        "--bin",
        metavar="PATH",
        default=None,
        help="clio binary to export with; defaults to CLIO_BIN, then a built target",
    )
    parser.add_argument(
        "--out",
        metavar="PATH",
        required=True,
        help="record file to write; preset packs land in its packs/ directory",
    )
    parser.add_argument(
        "--date",
        metavar="YYYY-MM-DD",
        default=None,
        help="measurement date recorded in every row; defaults to today (UTC)",
    )
    args = parser.parse_args(argv)

    date = args.date or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    out_path = Path(args.out)
    try:
        binary = _binary(args.bin)
        # Load the tokenizer before anything is written, so a missing package
        # leaves no record and no fixture behind.
        tokenizer = load()
        catalog_hash, rows = measure(binary, out_path.parent / "packs", tokenizer, date)
        output = _render(binary, out_path, catalog_hash, rows, tokenizer)
        try:
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(output, encoding="utf-8")
        except OSError as exc:
            raise BudgetError(f"cannot write record: {exc}") from None
    except BudgetError as exc:
        sys.stderr.write(f"mcp-preset-rows: {exc}\n")
        return 2

    for row in rows:
        sys.stdout.write(
            f"{row['preset']}: {row['bytes']} bytes, {row['tokens']} tokens\n"
        )
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
