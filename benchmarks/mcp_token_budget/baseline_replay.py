#!/usr/bin/env python3
"""Replay one MCP client fixture and report its per-turn byte and token counts.

A replay fixture records the tool definitions one client received on each turn.
This module reads one fixture, canonicalizes each turn's ``tools`` array, and
returns the canonical byte length and the pinned-tokenizer count for that turn.
The canonical form is sorted keys, ``(",", ":")`` separators, and UTF-8, so the
formatting of the file on disk never changes a number.

The whole result is built in memory before anything is printed, so a failure
exits 2 with ``mcp-baseline-replay: <message>`` on stderr and zero bytes on
stdout.
"""

import argparse
import json
import sys
from pathlib import Path

from offline_tokenizer import BudgetError, load

FIXTURE_FORMAT = "am_mcp_client_replay"
FIXTURE_VERSION = 1


def canonical_bytes(value) -> bytes:
    """Serialize ``value`` to the canonical bytes measured for one turn."""
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def read_fixture(path):
    """Return the parsed fixture object from ``path``, or raise BudgetError."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        raise BudgetError(f"cannot read fixture: {exc}") from None
    try:
        fixture = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        raise BudgetError(f"fixture is not valid UTF-8: {exc}") from None
    except json.JSONDecodeError as exc:
        raise BudgetError(f"fixture is not valid JSON: {exc}") from None
    if not isinstance(fixture, dict):
        raise BudgetError(f"fixture must be a JSON object, got {type(fixture).__name__}")
    if fixture.get("fixture_format") != FIXTURE_FORMAT:
        raise BudgetError(
            f"unsupported fixture format: {fixture.get('fixture_format')!r}, "
            f"expected {FIXTURE_FORMAT!r}"
        )
    if fixture.get("fixture_version") != FIXTURE_VERSION:
        raise BudgetError(
            f"unsupported fixture version: {fixture.get('fixture_version')!r}, "
            f"expected {FIXTURE_VERSION!r}"
        )
    turns = fixture.get("turns")
    if not isinstance(turns, list) or not turns:
        raise BudgetError("fixture has no turns")
    return fixture


def replay(fixture, tokenizer):
    """Return one result per turn, with the canonical bytes, byte length, and tokens."""
    results = []
    seen = set()
    for entry in fixture["turns"]:
        if not isinstance(entry, dict):
            raise BudgetError(f"turn entry must be a JSON object: {entry!r}")
        turn = entry.get("turn")
        if not isinstance(turn, int) or isinstance(turn, bool) or turn < 1:
            raise BudgetError(f"turn number must be a positive integer: {turn!r}")
        if turn in seen:
            raise BudgetError(f"duplicate turn: {turn}")
        seen.add(turn)
        payload = entry.get("payload")
        if not isinstance(payload, dict):
            raise BudgetError(f"turn {turn} has no payload object")
        tools = payload.get("tools")
        if not isinstance(tools, list) or not tools:
            raise BudgetError(f"turn {turn} has no tools")
        canonical = canonical_bytes(tools)
        results.append(
            {
                "turn": turn,
                "canonical_bytes": len(canonical),
                "tokens": tokenizer.count(canonical.decode("utf-8")),
            }
        )
    return results


def replay_file(path, tokenizer=None):
    """Read ``path`` and replay it; return ``(fixture, results)``."""
    fixture = read_fixture(path)
    return fixture, replay(fixture, tokenizer or load())


def _render(fixture, results) -> str:
    """Return the complete replay report as one string."""
    document = {
        "fixture_id": fixture.get("fixture_id"),
        "client": fixture.get("client"),
        "client_version": fixture.get("client_version"),
        "delivery": fixture.get("delivery"),
        "turns": results,
    }
    return json.dumps(document, ensure_ascii=False, indent=2) + "\n"


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this tool's stderr format."""

    def error(self, message):
        sys.stderr.write(f"mcp-baseline-replay: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Replay ``--fixture`` and print the per-turn rows; return the exit code."""
    parser = _Parser(
        prog="baseline_replay.py",
        description="Replay one MCP client fixture and print its per-turn byte and token counts.",
    )
    parser.add_argument(
        "--fixture",
        metavar="PATH",
        required=True,
        help="replay fixture file to measure",
    )
    args = parser.parse_args(argv)

    try:
        fixture, results = replay_file(args.fixture)
        output = _render(fixture, results)
    except BudgetError as exc:
        sys.stderr.write(f"mcp-baseline-replay: {exc}\n")
        return 2

    sys.stdout.write(output)
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
