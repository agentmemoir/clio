#!/usr/bin/env python3
"""Record the measured byte and token cost of the discovery flag on one preset.

The same binary is started twice as ``clio mcp stdio --tools <preset>
--discovery off`` and then ``on``. Each run answers ``initialize`` and
``tools/list`` on stdin/stdout, and only the served ``result.tools`` array is
measured: it is canonicalized with the helpers of ``preset_rows.py``, so its
bytes give the byte length, the cl100k_base token count, and the sha256 that
ties a row back to the exact surface the server advertised. The three discovery
tools are the only difference between the two runs, so the record's ``cost`` is
their on-minus-off size, derived here and never read from an input. The
``comparability`` block names the build both rows share and the frozen records
that predate the flag and must not be compared with this pair.

Tokens come from the repo-pinned offline tokenizer, so this script performs no
network I/O and never substitutes an estimate. Every refused measurement exits 2
with ``mcp-discovery-rows: <message>`` on stderr and writes no record.
"""

import argparse
import hashlib
import json
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
DISCOVERY_TOOLS = ("tool_search", "tool_describe", "tool_invoke")
FLAGS = ("off", "on")
COMMAND_TEMPLATE = (
    "python3 benchmarks/mcp_token_budget/discovery_rows.py --bin {binary} "
    "--preset {preset} --commit {commit} --out {out}"
)
NORMALIZATION = "canonical bytes of the served tools/list tools array only"
PRE_FLAG_PATHS = (
    "benchmarks/mcp_token_budget/presets/frozen-v1.json",
    "benchmarks/mcp_token_budget/baseline/frozen-v1.json",
)
COMPARABILITY_NOTE = (
    "Both rows were served by the same build, so the cost is the discovery flag "
    "alone. The pre-flag records listed here predate the flag and are not "
    "comparable with either row."
)
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
    """Return the commit the binary was built from, requiring it to resolve."""
    commit = (explicit or "").strip()
    if not commit:
        raise BudgetError("build identity has no commit")
    try:
        completed = subprocess.run(
            ["git", "cat-file", "-e", f"{commit}^{{commit}}"],
            cwd=str(_REPO_ROOT),
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
    except OSError as exc:
        raise BudgetError(f"cannot resolve build commit {commit}: {exc}") from None
    if completed.returncode != 0:
        detail = completed.stderr.strip() or "no stderr"
        raise BudgetError(
            f"build commit {commit} does not resolve in this checkout: {detail}"
        )
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
    """Index two rows by flag, requiring exactly one ``off`` and one ``on`` row."""
    flags = [row.get("flag") if isinstance(row, dict) else None for row in rows]
    if len(flags) != len(FLAGS) or set(flags) != set(FLAGS):
        raise BudgetError("the rows must hold one 'off' row and one 'on' row")
    return dict(zip(flags, rows))


def _names(tools, flag, preset):
    """Return the tool names of one capture, requiring one unique name per tool."""
    names = []
    for tool in tools:
        name = tool.get("name") if isinstance(tool, dict) else None
        if not isinstance(name, str) or not name:
            raise BudgetError(
                f"preset {preset!r} served a tool with no name with the flag {flag}"
            )
        names.append(name)
    if len(set(names)) != len(names):
        raise BudgetError(
            f"preset {preset!r} served a duplicate tool name with the flag {flag}"
        )
    return names


def check_discovery_tools(preset, off_tools, on_tools):
    """Require the on capture to add exactly the three discovery tools."""
    off_names = _names(off_tools, "off", preset)
    on_names = _names(on_tools, "on", preset)
    leaked = sorted(set(off_names) & set(DISCOVERY_TOOLS))
    if leaked:
        raise BudgetError(
            f"preset {preset!r} served {', '.join(leaked)} with the flag off"
        )
    missing = sorted(set(DISCOVERY_TOOLS) - set(on_names))
    if missing:
        raise BudgetError(
            f"preset {preset!r} did not serve {', '.join(missing)} with the flag on"
        )
    on_only = sorted(set(on_names) - set(off_names))
    if on_only != sorted(DISCOVERY_TOOLS):
        added = ", ".join(on_only) or "no tools"
        expected = ", ".join(DISCOVERY_TOOLS)
        raise BudgetError(
            f"preset {preset!r} added {added} with the flag on, expected {expected}"
        )


def check_tool_counts(preset, rows, off_tools, on_tools):
    """Require the recorded counts and the three discovery tools."""
    indexed = _by_flag(rows)
    expected = OFF_TOOL_COUNTS.get(preset)
    if expected is None:
        known = ", ".join(sorted(OFF_TOOL_COUNTS))
        raise BudgetError(
            f"preset {preset!r} has no known off tool count; known presets: {known}"
        )
    if indexed["off"]["tools"] != expected:
        raise BudgetError(
            f"preset {preset!r} served {indexed['off']['tools']} tools with the "
            f"flag off, expected {expected}"
        )
    wanted_on = expected + len(DISCOVERY_TOOLS)
    if indexed["on"]["tools"] != wanted_on:
        raise BudgetError(
            f"preset {preset!r} served {indexed['on']['tools']} tools with the "
            f"flag on, expected {wanted_on}"
        )
    check_discovery_tools(preset, off_tools, on_tools)


def cost(rows):
    """Return the discovery flag's cost as on-minus-off bytes and tokens."""
    indexed = _by_flag(rows)
    return {
        "bytes": indexed["on"]["bytes"] - indexed["off"]["bytes"],
        "tokens": indexed["on"]["tokens"] - indexed["off"]["tokens"],
    }


def _record_date(path):
    """Return the single row date of one frozen record, or raise BudgetError."""
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise BudgetError(f"cannot read pre-flag record {path.name}: {exc}") from None
    dates = {
        row.get("date")
        for row in document.get("rows", [])
        if isinstance(row, dict) and row.get("date")
    }
    if len(dates) != 1:
        raise BudgetError(f"pre-flag record {path.name} has no single row date")
    return dates.pop()


def pre_flag_records():
    """Return the frozen records that predate the flag, with their row date."""
    return [
        {"path": path, "date": _record_date(_REPO_ROOT / path)}
        for path in PRE_FLAG_PATHS
    ]


def comparability(rows, build):
    """Return the block that pins the row pair and rules out older records."""
    indexed = _by_flag(rows)
    return {
        "pair": [indexed["off"]["flag"], indexed["on"]["flag"]],
        "shared_build": build,
        "not_comparable_with": pre_flag_records(),
        "note": COMPARABILITY_NOTE,
    }


def measure(binary, preset, tokenizer):
    """Capture both flag states and return the two measured rows."""
    off_tools = _serve(binary, preset, "off")
    on_tools = _serve(binary, preset, "on")
    rows = [
        row("off", off_tools, tokenizer),
        row("on", on_tools, tokenizer),
    ]
    check_tool_counts(preset, rows, off_tools, on_tools)
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
                binary="<clio-binary>",
                preset=preset,
                commit=build["commit"],
                out=_display(out_path),
            ),
            "normalization": NORMALIZATION,
            "encoder": tokenizer.encoding,
            "tokenizer": f"tiktoken=={tokenizer.provenance()['version']}",
        },
        "rows": rows,
        "cost": cost(rows),
        "comparability": comparability(rows, build),
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
        required=True,
        help="commit the binary was built from; must resolve in this checkout",
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
