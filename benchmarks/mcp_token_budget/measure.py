#!/usr/bin/env python3
"""Print every MCP tool-schema token-budget row for one schema pack.

The input is the `am_tool_schema_pack` object that `clio mcp schema-export`
writes to stdout, or a pack file named with `--pack PATH`. The pack is
canonicalized once (sorted keys, no whitespace, UTF-8), and the canonical bytes
are the only thing measured: their byte length and their cl100k_base token
count. That byte string is exactly what the binary prints, minus its trailing
newline, so the numbers describe the shipped tool surface and not a re-render
of it.

Tokens are counted with the repo-pinned offline tokenizer, so this script
performs no network I/O, downloads nothing, and never substitutes an estimate.
The whole document is built in memory and written to stdout once, so a failure
can never leave partial rows behind: every failure exits 2 with
``mcp-token-budget: <message>`` on stderr and zero bytes on stdout.
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

from offline_tokenizer import BudgetError, load

PACK_FORMAT = "am_tool_schema_pack"
OUTPUT_FORMAT = "clio_mcp_token_budget"
OUTPUT_FORMAT_VERSION = 1
LABELS = frozenset(
    {"Measured", "Vendor-measured", "External", "Inferred", "Decision", "Unverified"}
)
UNITS = frozenset({"bytes", "tokens"})

#: Repo-relative asset path reported in the output; never absolute, never host-specific.
ASSET_PATH = "benchmarks/mcp_token_budget/assets/cl100k_base.tiktoken"

_ASSET_ON_DISK = Path(__file__).resolve().parent / "assets" / "cl100k_base.tiktoken"


def canonical_bytes(value) -> bytes:
    """Serialize ``value`` to the bytes ``clio mcp schema-export`` prints, minus its newline."""
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def _read_pack(path):
    """Return the raw pack bytes from ``path``, or from stdin when it is None."""
    if path is None:
        return sys.stdin.buffer.read()
    try:
        return Path(path).read_bytes()
    except OSError as exc:
        raise BudgetError(f"cannot read pack: {exc}") from None


def _parse_pack(raw):
    """Decode and validate the pack object; raise BudgetError on anything unexpected."""
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise BudgetError(f"pack is not valid UTF-8: {exc}") from None
    try:
        pack = json.loads(text)
    except json.JSONDecodeError as exc:
        raise BudgetError(f"pack is not valid JSON: {exc}") from None
    if not isinstance(pack, dict):
        raise BudgetError(f"pack must be a JSON object, got {type(pack).__name__}")
    if pack.get("format") != PACK_FORMAT:
        raise BudgetError(
            f"unsupported pack format: {pack.get('format')!r}, expected {PACK_FORMAT!r}"
        )
    tools = pack.get("tools")
    if not isinstance(tools, list) or not tools:
        raise BudgetError("pack has no tools")
    return pack


def _row(row_id, value, unit, digest):
    """Build one measurement row, refusing a unit or label outside the documented sets."""
    label = "Measured"
    if unit not in UNITS:
        raise BudgetError(f"unsupported unit: {unit!r}")
    if label not in LABELS:
        raise BudgetError(f"unsupported label: {label!r}")
    return {
        "row": row_id,
        "value": value,
        "unit": unit,
        "label": label,
        "input_sha256": digest,
    }


def _asset_sha256():
    """Hash the encoding asset that ``load()`` has already verified."""
    try:
        return hashlib.sha256(_ASSET_ON_DISK.read_bytes()).hexdigest()
    except OSError as exc:
        raise BudgetError(f"missing required encoding asset: {exc}") from None


def _render(pack, tokenizer) -> str:
    """Return the complete output document for ``pack`` as one string."""
    canonical = canonical_bytes(pack)
    digest = hashlib.sha256(canonical).hexdigest()
    provenance = tokenizer.provenance()
    document = {
        "format": OUTPUT_FORMAT,
        "format_version": OUTPUT_FORMAT_VERSION,
        "encoding": tokenizer.encoding,
        "tokenizer": {
            "implementation": provenance["implementation"],
            "version": provenance["version"],
        },
        "asset": {"path": ASSET_PATH, "sha256": _asset_sha256()},
        "rows": [
            _row("pack.bytes", len(canonical), "bytes", digest),
            _row("pack.tokens", tokenizer.count(canonical.decode("utf-8")), "tokens", digest),
        ],
    }
    return json.dumps(document, ensure_ascii=False, indent=2) + "\n"


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this tool's stderr format."""

    def error(self, message):
        sys.stderr.write(f"mcp-token-budget: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Measure ``--pack`` (or stdin) and print the rows; return the process exit code."""
    parser = _Parser(
        prog="measure.py",
        description="Print token-budget rows for an am_tool_schema_pack JSON object.",
    )
    parser.add_argument(
        "--pack",
        metavar="PATH",
        default=None,
        help="pack file to measure; reads stdin when omitted",
    )
    args = parser.parse_args(argv)

    try:
        pack = _parse_pack(_read_pack(args.pack))
        # Load the tokenizer before anything is written: a missing package or a
        # missing/corrupt asset must fail with no rows on stdout.
        tokenizer = load()
        output = _render(pack, tokenizer)
    except BudgetError as exc:
        sys.stderr.write(f"mcp-token-budget: {exc}\n")
        return 2

    sys.stdout.write(output)
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
