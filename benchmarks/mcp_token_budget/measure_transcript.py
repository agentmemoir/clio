#!/usr/bin/env python3
"""Write one cost record for a recorded MCP client session pair.

Each input is an `am_mcp_client_session` fixture with one `initialize` and one
`tools/call` exchange; a session total sums the canonical byte sizes of its
response lines, and a payload copy counts each appearance of the payload in the
call result. Token counts need the pinned tokenizer package; when it is
missing, the token rows carry a null value and the label `Not measured`.
Two delta rows report the after-minus-before change, and a delta row's
`input_sha256` is the digest of the two measured input digests joined in
measurement order.
"""

import argparse
import hashlib
import json
import platform
import sys
from pathlib import Path

from baseline_replay import canonical_bytes
from offline_tokenizer import BudgetError, load

FIXTURE = ("am_mcp_client_session", 1)


def _row(row_id, value, unit, digest, label="Measured", reason=None):
    row = {"row": row_id, "value": value, "unit": unit, "label": label, "input_sha256": digest}
    return row if reason is None else row | {"reason": reason}


def _measure(path):
    try:
        raw = Path(path).read_bytes()
        session = json.loads(raw.decode("utf-8"))
    except OSError as exc:
        raise BudgetError(f"cannot read session fixture: {exc}") from None
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise BudgetError(f"session fixture is not valid JSON: {exc}") from None
    if not isinstance(session, dict):
        raise BudgetError("session fixture must be a JSON object")
    if (session.get("fixture_format"), session.get("fixture_version")) != FIXTURE:
        raise BudgetError("unsupported fixture format or version")
    exchanges = session.get("exchanges")
    if not isinstance(exchanges, list) or len(exchanges) != 2:
        raise BudgetError("session fixture must hold exactly two exchanges")
    parsed, lines = [], []
    for exchange in exchanges:
        line = exchange.get("response") if isinstance(exchange, dict) else None
        if not isinstance(line, str):
            raise BudgetError("exchange has no response line")
        try:
            parsed.append(json.loads(line))
        except json.JSONDecodeError as exc:
            raise BudgetError(f"response line is not valid JSON: {exc}") from None
        lines.append(canonical_bytes(parsed[-1]))
    response = parsed[1]
    if (not isinstance(response, dict) or response.get("jsonrpc") != "2.0"
            or not isinstance(response.get("result"), dict)):
        raise BudgetError("tools/call response is not a JSON-RPC result object")
    result = response["result"]
    structured = result.get("structuredContent")
    copies = 1 if structured is not None else 0
    content = result.get("content")
    text = content[0].get("text") if isinstance(content, list) and content else None
    try:
        decoded = json.loads(text) if isinstance(text, str) else None
    except json.JSONDecodeError:
        decoded = None
    if decoded is not None and decoded == structured:
        copies += 1
    joined = b"".join(lines)
    return {
        "bytes": len(joined), "sha256": hashlib.sha256(joined).hexdigest(),
        "call_bytes": len(lines[1]), "call_sha256": hashlib.sha256(lines[1]).hexdigest(),
        "payload_copies": copies, "text": joined.decode("utf-8"),
    }


def _render(before, after, tokenizer, tokens, reason):
    if tokenizer is None:
        tokenizer_block = {"status": "not measured", "reason": reason}
    else:
        tokenizer_block = {"package": "tiktoken", "encoding": tokenizer.encoding,
                           "version": tokenizer.provenance()["version"],
                           "interpreter": f"{platform.python_implementation()} "
                                          f"{platform.python_version()}"}
    pair_digest = hashlib.sha256(
        (before["sha256"] + after["sha256"]).encode("ascii")).hexdigest()
    rows = []
    for side, info in (("before", before), ("after", after)):
        rows += [
            _row(f"session.{side}.bytes", info["bytes"], "bytes", info["sha256"]),
            _row(f"call.{side}.bytes", info["call_bytes"], "bytes", info["call_sha256"]),
            _row(f"call.{side}.payload_copies", info["payload_copies"], "count",
                 info["call_sha256"]),
            _row(f"session.{side}.tokens", tokens[side], "tokens", info["sha256"],
                 label="Not measured" if tokens[side] is None else "Measured", reason=reason),
        ]
    delta_tokens = None if tokenizer is None else tokens["after"] - tokens["before"]
    rows += [
        _row("session.delta.bytes", after["bytes"] - before["bytes"], "bytes", pair_digest),
        _row("session.delta.tokens", delta_tokens, "tokens", pair_digest,
             label="Not measured" if delta_tokens is None else "Measured", reason=reason),
    ]
    document = {"format": "clio_mcp_transcript_cost", "format_version": 1,
                "tokenizer": tokenizer_block, "rows": rows}
    return json.dumps(document, ensure_ascii=False, indent=2) + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="measure_transcript.py")
    for name in ("before", "after", "out"):
        parser.add_argument(f"--{name}", metavar="PATH", required=True)
    args = parser.parse_args(argv)
    try:
        before, after = _measure(args.before), _measure(args.after)
    except BudgetError as exc:
        sys.stderr.write(f"mcp-transcript-cost: {exc}\n")
        return 2
    try:
        tokenizer, reason = load(), None
    except BudgetError as exc:
        tokenizer, reason = None, str(exc)
    tokens = {side: None if tokenizer is None else tokenizer.count(info["text"])
              for side, info in (("before", before), ("after", after))}
    Path(args.out).write_text(_render(before, after, tokenizer, tokens, reason), encoding="utf-8")
    print(f"before bytes: {before['bytes']}\nafter bytes:  {after['bytes']}\n"
          f"delta bytes:  {after['bytes'] - before['bytes']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
