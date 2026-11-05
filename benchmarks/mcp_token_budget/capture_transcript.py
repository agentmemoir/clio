#!/usr/bin/env python3
"""Capture a deterministic two-exchange ``tools/call`` session from a binary.

The capture drives one real ``clio mcp stdio`` process, records the raw request
line and the raw response line for each exchange, and writes them as strings so
a test can replay the exact bytes. Nothing is re-serialized: the recorded text
is the text that crossed the pipe.

The run is all or nothing. A non-zero exit, any stderr output, or a response
count other than two exits 2 with a message on stderr and writes no file, so a
partial or logged session can never become a fixture.

Each fixture also records a ``capture`` block: the source commit, the sha256 of
the executable that ran, and the UTC capture time. A recorded session can then
be tied to the exact source and binary that produced it.
"""

import argparse
import hashlib
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

FIXTURE_FORMAT = "am_mcp_client_session"
FIXTURE_VERSION = 1
CLIENT = "clio mcp stdio"
PROTOCOL_VERSION = "2025-11-25"
SERVER_ARGS = ["mcp", "stdio", "--db", "sqlite::memory:"]
TIMEOUT_SECONDS = 120
EXPECTED_EXCHANGES = 2

#: Tools that read no store state, so the recorded payload is reproducible.
TOOL_NAME = "ranking_env_get"


class CaptureError(RuntimeError):
    """Raised when the session cannot be recorded as a fixture."""


def compact(value) -> str:
    """Serialize ``value`` as one compact JSON line."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def frames() -> list[str]:
    """Return the request lines, in order, for one capture session."""
    return [
        compact(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "clio-mcp-transcript", "version": "1"},
                },
            }
        ),
        compact(
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": TOOL_NAME, "arguments": {}},
            }
        ),
    ]


def source_commit(source_dir: Path) -> str:
    """Return the commit that names the captured source tree."""
    result = subprocess.run(
        ["git", "-C", str(source_dir), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        timeout=TIMEOUT_SECONDS,
        check=False,
    )
    if result.returncode != 0:
        raise CaptureError(
            f"`git rev-parse HEAD` in {source_dir} exited {result.returncode}: "
            f"{result.stderr.strip()[:400]}"
        )
    commit = result.stdout.strip()
    if len(commit) != 40 or any(char not in "0123456789abcdef" for char in commit):
        raise CaptureError(f"`git rev-parse HEAD` returned {commit!r}")
    return commit


def binary_sha256(binary: str) -> str:
    """Return the sha256 digest of the executable that ran the session."""
    path = Path(binary)
    if not path.is_file():
        raise CaptureError(f"binary is not a file: {binary}")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def captured_at() -> str:
    """Return the capture time as UTC ISO-8601."""
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def client_version(binary: str) -> str:
    """Return the first line of ``<binary> --version``."""
    result = subprocess.run(
        [binary, "--version"],
        capture_output=True,
        text=True,
        timeout=TIMEOUT_SECONDS,
        check=False,
    )
    if result.returncode != 0:
        raise CaptureError(f"`{binary} --version` exited {result.returncode}")
    first = result.stdout.splitlines()
    if not first or not first[0].strip():
        raise CaptureError(f"`{binary} --version` printed no version line")
    return first[0].strip()


def run_session(binary: str, requests: list[str]) -> tuple[str, str]:
    """Run the session and return its stdout and stderr."""
    stdin_text = "\n".join(requests) + "\n"
    try:
        result = subprocess.run(
            [binary, *SERVER_ARGS],
            input=stdin_text,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        raise CaptureError(f"session did not finish within {TIMEOUT_SECONDS}s") from None
    except OSError as exc:
        raise CaptureError(f"cannot run {binary}: {exc}") from None
    if result.returncode != 0:
        raise CaptureError(f"session exited {result.returncode}")
    if result.stderr.strip():
        raise CaptureError(f"session wrote to stderr: {result.stderr.strip()[:400]}")
    return result.stdout, result.stderr


def build_fixture(binary: str, requests: list[str], source_dir: Path) -> dict:
    """Return the fixture object for one captured session."""
    stdout, _stderr = run_session(binary, requests)
    responses = [line for line in stdout.split("\n") if line.strip()]
    if len(responses) != EXPECTED_EXCHANGES:
        raise CaptureError(
            f"expected {EXPECTED_EXCHANGES} response lines, got {len(responses)}"
        )
    for request, response in zip(requests, responses):
        parsed = json.loads(response)
        if parsed.get("id") != json.loads(request).get("id"):
            raise CaptureError("response id does not match its request id")
    return {
        "fixture_format": FIXTURE_FORMAT,
        "fixture_version": FIXTURE_VERSION,
        "client": CLIENT,
        "client_version": client_version(binary),
        "delivery": "stdio",
        "command": [binary, *SERVER_ARGS],
        "capture": {
            "source_commit": source_commit(source_dir),
            "binary_sha256": binary_sha256(binary),
            "captured_at": captured_at(),
        },
        "exchanges": [
            {"request": request, "response": response}
            for request, response in zip(requests, responses)
        ],
    }


def main(argv=None) -> int:
    """Capture one session; return 0 on success and 2 on any failure."""
    parser = argparse.ArgumentParser(
        prog="capture_transcript.py",
        description="Record one deterministic clio MCP stdio tools/call session.",
    )
    parser.add_argument("--binary", required=True, help="clio binary to drive")
    parser.add_argument("--out", required=True, help="fixture file to write")
    parser.add_argument(
        "--source-dir",
        default=".",
        help="directory whose git HEAD names the captured source (default: .)",
    )
    args = parser.parse_args(argv)

    out = Path(args.out)
    try:
        fixture = build_fixture(args.binary, frames(), Path(args.source_dir))
        text = json.dumps(fixture, ensure_ascii=False, indent=2) + "\n"
    except CaptureError as exc:
        sys.stderr.write(f"mcp-capture-transcript: {exc}\n")
        return 2
    try:
        out.write_text(text, encoding="utf-8")
    except OSError as exc:
        sys.stderr.write(f"mcp-capture-transcript: cannot write fixture: {exc}\n")
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
