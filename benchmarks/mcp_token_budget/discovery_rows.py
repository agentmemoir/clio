#!/usr/bin/env python3
"""Record the measured byte and token cost of the discovery flag on one preset.

The same binary is started twice as ``clio mcp stdio --tools <preset>
--discovery off`` and then ``on``. Each run answers ``initialize`` and
``tools/list`` on stdin/stdout, and only the served ``result.tools`` array is
measured: it is canonicalized with the helpers of ``preset_rows.py``, so its
bytes give the byte length, the cl100k_base token count, and the sha256 that
ties a row back to the exact surface the server advertised. The three discovery
tools are the only difference between the two runs, so the record's ``cost`` is
their on-minus-off size, derived here and never read from an input.

Tokens come from the repo-pinned offline tokenizer, so this script performs no
network I/O and never substitutes an estimate. Every failure exits 2 with
``mcp-discovery-rows: <message>`` on stderr and writes no record.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

from offline_tokenizer import BudgetError, load
from preset_rows import _REPO_ROOT, _binary, _display, canonical_bytes

RECORD_FORMAT = "am_mcp_discovery_surface"
RECORD_FORMAT_VERSION = 1
RECORD_ID = "discovery-surface-v1"
DELIVERY = "served-tools-list"
DEFAULT_PRESET = "minimal"
OFF_TOOL_COUNTS = {"minimal": 6}
DISCOVERY_TOOL_COUNT = 3
FLAGS = ("off", "on")
COMMAND_TEMPLATE = (
    "python3 benchmarks/mcp_token_budget/discovery_rows.py --bin {binary} --out {out}"
)
NORMALIZATION = "canonical bytes of the served tools/list tools array only"
_PROTOCOL_VERSION = "2025-11-25"
_CAPTURE_TIMEOUT = 60
_VERSION_TIMEOUT = 30


def _serve(binary, preset, flag):
    """Start one stdio server, ask for ``tools/list``, and return the tools array."""
    command = [
        str(binary), "mcp", "stdio", "--tools", preset, "--discovery", flag
    ]
    requests = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": _PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "discovery-rows", "version": "0"},
            },
        },
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    ]
    payload = "".join(
        json.dumps(request, separators=(",", ":")) + "\n" for request in requests
    )
    try:
        process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
    except OSError as exc:
        raise BudgetError(f"cannot run {binary}: {exc}") from None
    try:
        stdout, stderr = process.communicate(payload, timeout=_CAPTURE_TIMEOUT)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        raise BudgetError(
            f"capture timed out for --discovery {flag} on preset {preset!r}"
        ) from None
    if process.returncode != 0:
        detail = (stderr or "").strip() or "no stderr"
        raise BudgetError(
            f"capture failed for --discovery {flag} on preset {preset!r}: {detail}"
        )
    tools = _tools_from(stdout, preset, flag)
    return tools


def _tools_from(stdout, preset, flag):
    """Return the ``tools/list`` result array from one server's stdout."""
    for line in stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(message, dict) or message.get("id") != 2:
            continue
        result = message.get("result")
        if not isinstance(result, dict):
            continue
        tools = result.get("tools")
        if isinstance(tools, list):
            return tools
    raise BudgetError(
        f"no tools/list result for --discovery {flag} on preset {preset!r}"
    )


def _version(binary):
    """Return the binary's version string, or raise BudgetError."""
    try:
        completed = subprocess.run(
            [str(binary), "--version"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=_VERSION_TIMEOUT,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise BudgetError(f"cannot read the binary version: {exc}") from None
    version = completed.stdout.strip()
    if completed.returncode != 0 or not version:
        detail = completed.stderr.strip() or "no stdout"
        raise BudgetError(f"cannot read the binary version: {detail}")
    return version


def _commit(explicit):
    """Return the measured commit: ``explicit`` or the checkout's ``HEAD``."""
    if explicit:
        commit = explicit.strip()
    else:
        try:
            completed = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=str(_REPO_ROOT),
                capture_output=True,
                text=True,
                encoding="utf-8",
            )
        except OSError as exc:
            raise BudgetError(f"cannot read the build commit: {exc}") from None
        if completed.returncode != 0:
            detail = completed.stderr.strip() or "no stderr"
            raise BudgetError(f"cannot read the build commit: {detail}")
        commit = completed.stdout.strip()
    if not commit:
        raise BudgetError("build identity has no commit")
    return commit


def build_identity(commit, version):
    """Validate and return the build identity the measurement belongs to."""
    if not isinstance(commit, str) or not commit.strip():
        raise BudgetError("build identity has no commit")
    if not isinstance(version, str) or not version.strip():
        raise BudgetError("cannot read the binary version string")
    return {"commit": commit.strip(), "version": version.strip()}


def row(flag, tools, tokenizer):
    """Build one measured row from a served tools array."""
    canonical = canonical_bytes(tools)
    return {
        "flag": flag,
        "label": "Measured",
        "tools": len(tools),
        "bytes": len(canonical),
        "tokens": tokenizer.count(canonical.decode("utf-8")),
        "input_sha256": hashlib.sha256(canonical).hexdigest(),
    }


def _by_flag(rows):
    """Index two rows by flag, requiring exactly one ``off`` and one ``on``."""
    indexed = {row["flag"]: row for row in rows}
    if set(indexed) != set(FLAGS):
        raise BudgetError("the rows must hold one 'off' row and one 'on' row")
    return indexed


def check_tool_counts(preset, rows):
    """Require the recorded counts: the preset's off count and off plus three."""
    indexed = _by_flag(rows)
    expected = OFF_TOOL_COUNTS.get(preset)
    if expected is not None and indexed["off"]["tools"] != expected:
        raise BudgetError(
            f"preset {preset!r} served {indexed['off']['tools']} tools with the "
            f"flag off, expected {expected}"
        )
    wanted_on = indexed["off"]["tools"] + DISCOVERY_TOOL_COUNT
    if indexed["on"]["tools"] != wanted_on:
        raise BudgetError(
            f"preset {preset!r} served {indexed['on']['tools']} tools with the "
            f"flag on, expected {wanted_on}"
        )


def cost(rows):
    """Return the discovery flag's cost as on-minus-off bytes and tokens."""
    indexed = _by_flag(rows)
    return {
        "bytes": indexed["on"]["bytes"] - indexed["off"]["bytes"],
        "tokens": indexed["on"]["tokens"] - indexed["off"]["tokens"],
    }


def measure(binary, preset, tokenizer):
    """Capture both flag states and return the two measured rows."""
    off_tools = _serve(binary, preset, "off")
    on_tools = _serve(binary, preset, "on")
    rows = [
        row("off", off_tools, tokenizer),
        row("on", on_tools, tokenizer),
    ]
    check_tool_counts(preset, rows)
    return rows


def render(out_path, preset, date, build, rows, tokenizer) -> str:
    """Return the complete record document as one string."""
    document = {
        "format": RECORD_FORMAT,
        "format_version": RECORD_FORMAT_VERSION,
        "record_id": RECORD_ID,
        "preset": preset,
        "date": date,
        "delivery": DELIVERY,
        "build": build,
        "measurement": {
            "command": COMMAND_TEMPLATE.format(
                binary="<clio-binary>", out=_display(out_path)
            ),
            "normalization": NORMALIZATION,
            "encoder": tokenizer.encoding,
            "tokenizer": f"tiktoken=={tokenizer.provenance()['version']}",
        },
        "rows": rows,
        "cost": cost(rows),
    }
    return (
        json.dumps(document, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    )


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this tool's stderr format."""

    def error(self, message):
        sys.stderr.write(f"mcp-discovery-rows: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Capture both flag states, write the record; return the exit code."""
    parser = _Parser(
        prog="discovery_rows.py",
        description="Record the discovery flag's cost on one MCP preset.",
    )
    parser.add_argument(
        "--bin",
        metavar="PATH",
        default=None,
        help="clio binary to capture with; defaults to CLIO_BIN, then a built target",
    )
    parser.add_argument(
        "--out",
        metavar="PATH",
        required=True,
        help="record file to write",
    )
    parser.add_argument(
        "--preset",
        metavar="NAME",
        default=DEFAULT_PRESET,
        help=f"preset to capture twice; defaults to {DEFAULT_PRESET}",
    )
    parser.add_argument(
        "--commit",
        metavar="SHA",
        default=None,
        help="commit the binary was built from; defaults to git rev-parse HEAD",
    )
    parser.add_argument(
        "--date",
        metavar="YYYY-MM-DD",
        default=None,
        help="measurement date recorded in the record; defaults to today (UTC)",
    )
    args = parser.parse_args(argv)

    date = args.date or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    out_path = Path(args.out)
    try:
        binary = _binary(args.bin)
        # Load the tokenizer and read the build identity before anything is
        # captured, so a missing input leaves no record behind.
        tokenizer = load()
        build = build_identity(_commit(args.commit), _version(binary))
        rows = measure(binary, args.preset, tokenizer)
        output = render(out_path, args.preset, date, build, rows, tokenizer)
        try:
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(output, encoding="utf-8")
        except OSError as exc:
            raise BudgetError(f"cannot write record: {exc}") from None
    except BudgetError as exc:
        sys.stderr.write(f"mcp-discovery-rows: {exc}\n")
        return 2

    for measured in rows:
        sys.stdout.write(
            f"{args.preset} --discovery {measured['flag']}: "
            f"{measured['tools']} tools, {measured['bytes']} bytes, "
            f"{measured['tokens']} tokens\n"
        )
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
