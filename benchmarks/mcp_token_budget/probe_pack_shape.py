#!/usr/bin/env python3
"""Probe whether the baseline-captured client resolves shared schema references.

The pack ships exactly one shape, and which one it ships is decided by evidence
rather than by preference. This probe produces that evidence: it reads the
client captures the frozen baseline names, and it reports what those captures
actually show about shared schema references, meaning local `#/...` pointers and
the `$defs` objects they resolve against.

The receipt it emits says only what was observed. A capture holding no local
`$ref` and no `$defs` gives the probe nothing to observe, so the status is
``not_run``: the client's support for shared references is unobserved, and an
unobserved client never counts as a pass. ``passed`` needs at least one local
`$ref` whose every target resolves inside the captured document, and ``failed``
is a local `$ref` whose target does not. The receipt names the fixture it read,
the client identity that fixture carries, its provenance, and the counts behind
the verdict, so a reader can tell a synthetic capture from a real client.

Nothing here runs a client, reaches the network, or reads the pack: it reads
committed capture bytes and hashes them, so two runs on one checkout agree. Any
unusable input exits 2 with ``pack-shape-probe: <message>`` on stderr, which is
never a pass, and a receipt prints to stdout once with ``--json``.
"""

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent

#: The capture record the probe reads its client fixtures from.
BASELINE_PATH = "benchmarks/mcp_token_budget/baseline/frozen-v1.json"
BASELINE_FILE = HERE / "baseline" / "frozen-v1.json"

#: The three statuses a receipt may carry, and nothing else.
STATUS_NOT_RUN = "not_run"
STATUS_PASSED = "passed"
STATUS_FAILED = "failed"
STATUSES = (STATUS_NOT_RUN, STATUS_PASSED, STATUS_FAILED)

#: Provenance stated for a capture this probe did not take from a live client.
PROVENANCE_SYNTHETIC = "synthetic-mock"
SYNTHETIC_CAPTURE = "synthetic-replay"

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class ProbeError(ValueError):
    """Raised when a capture the probe must read cannot be trusted."""


def _fail(message):
    raise ProbeError(f"pack-shape-probe: {message}")


def _pointer(root, reference):
    """Return the value ``reference`` names inside ``root``, or None.

    A local reference is resolved exactly as a JSON Pointer: ``#`` is the whole
    document, ``#/`` starts a token path, and ``~1`` and ``~0`` stand for a
    slash and a tilde. An external or malformed reference resolves to nothing,
    which the caller never counts as a shared schema reference.
    """
    if not isinstance(reference, str) or not reference.startswith("#"):
        return None
    path = reference[1:]
    if not path:
        return root
    if not path.startswith("/"):
        return None
    node = root
    for raw in path[1:].split("/"):
        token = raw.replace("~1", "/").replace("~0", "~")
        if isinstance(node, dict):
            if token not in node:
                return None
            node = node[token]
        elif isinstance(node, list):
            try:
                index = int(token)
            except ValueError:
                return None
            if index < 0 or index >= len(node):
                return None
            node = node[index]
        else:
            return None
    return node


def _local(reference) -> bool:
    """Whether ``reference`` points inside the document that holds it."""
    return (
        isinstance(reference, str)
        and reference.startswith("#")
        and (len(reference) == 1 or reference.startswith("#/"))
    )


def _observe(payload) -> dict:
    """Return the shared-reference counts one captured payload shows.

    A local reference is resolved against the captured payload and against every
    object above it that carries a `$defs`, so a schema that defines its own
    shared objects is read the way a client reads it. A reference none of those
    scopes resolve is the only thing counted as unresolved.
    """
    refs = 0
    defs = 0
    unresolved = 0

    def walk(node, scopes):
        nonlocal refs, defs, unresolved
        if isinstance(node, dict):
            if isinstance(node.get("$defs"), dict):
                defs += 1
                scopes = [*scopes, node]
            for key, value in node.items():
                if key == "$ref" and _local(value):
                    refs += 1
                    if not any(_pointer(scope, value) is not None for scope in scopes):
                        unresolved += 1
                walk(value, scopes)
        elif isinstance(node, list):
            for item in node:
                walk(item, scopes)

    walk(payload, [payload])
    return {
        "refs_observed": refs,
        "defs_observed": defs,
        "unresolved_refs": unresolved,
    }


def _provenance(capture) -> str:
    """The provenance stated for a capture kind; never a claim about a client."""
    if capture == SYNTHETIC_CAPTURE:
        return PROVENANCE_SYNTHETIC
    return f"unverified:{capture}"


def _client_identity(client, fixture_path, sha256) -> dict:
    """Return the identity block for one captured fixture."""
    return {
        "fixture": f"{BASELINE_PATH.rsplit('/', 1)[0]}/clients/{fixture_path.name}",
        "name": client["client"],
        "version": client["client_version"],
        "sha256": sha256,
        "provenance": _provenance(client.get("capture")),
        "fixture_version": client["fixture"]["version"],
    }


def _read_fixture(path, expected):
    """Return the parsed capture at ``path``, refusing a moved or unreadable file."""
    try:
        raw = path.read_bytes()
    except OSError as exc:
        _fail(f"cannot read capture {path.name}: {exc}")
    digest = hashlib.sha256(raw).hexdigest()
    if digest != expected:
        _fail(
            f"capture {path.name} digest is {digest}, "
            f"expected the recorded {expected}"
        )
    try:
        return json.loads(raw.decode("utf-8")), digest
    except UnicodeDecodeError as exc:
        _fail(f"capture {path.name} is not valid UTF-8: {exc}")
    except json.JSONDecodeError as exc:
        _fail(f"capture {path.name} is not valid JSON: {exc}")


def _clients(record) -> list:
    """Return the client entries of a baseline record, refusing an empty list."""
    clients = record.get("clients")
    if not isinstance(clients, list) or not clients:
        _fail("the baseline record names no client capture")
    for index, client in enumerate(clients):
        if not isinstance(client, dict) or not isinstance(client.get("client"), str):
            _fail(f"client entry {index} has no client name")
        if not isinstance(client.get("client_version"), str):
            _fail(f"client {client['client']!r} has no version")
        fixture = client.get("fixture")
        if not isinstance(fixture, dict) or not isinstance(fixture.get("name"), str):
            _fail(f"client {client['client']!r} names no fixture")
        if not isinstance(fixture.get("sha256"), str) or not SHA256_RE.match(fixture["sha256"]):
            _fail(f"client {client['client']!r} fixture digest is not 64 lowercase hex")
    return clients


def load_baseline(path=BASELINE_FILE) -> dict:
    """Return the baseline capture record at ``path``."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        _fail(f"cannot read the baseline record: {exc}")
    try:
        record = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        _fail(f"the baseline record is not valid UTF-8: {exc}")
    except json.JSONDecodeError as exc:
        _fail(f"the baseline record is not valid JSON: {exc}")
    if not isinstance(record, dict):
        _fail(f"the baseline record must be an object, got {type(record).__name__}")
    return record


def _status(observation) -> tuple:
    """Return the status and its plain-words reason for one observation."""
    refs = observation["refs_observed"]
    if refs == 0:
        observed_defs = observation["defs_observed"]
        extra = (
            f" and {observed_defs} local definition object(s)"
            if observed_defs
            else ""
        )
        return (
            STATUS_NOT_RUN,
            "the baseline-captured payload holds no shared schema reference"
            f"{extra}, so client support for shared schema references is unobserved",
        )
    if observation["unresolved_refs"]:
        return (
            STATUS_FAILED,
            f"the baseline-captured payload holds {refs} local reference(s) and "
            f"{observation['unresolved_refs']} of them do not resolve inside the "
            "captured document",
        )
    return (
        STATUS_PASSED,
        f"the baseline-captured payload holds {refs} local reference(s) and every "
        "one resolves inside the captured document",
    )


def receipt_id(client, status) -> str:
    """Return the stable receipt id derived from a fixture digest and a status."""
    return f"pack-shape-probe-{client['sha256'][:16]}-{status}"


def probe(path=BASELINE_FILE) -> dict:
    """Return the receipt for the client captures the baseline record names."""
    record = load_baseline(path)
    clients = _clients(record)
    examined = []
    identity = None
    total = {"refs_observed": 0, "defs_observed": 0, "unresolved_refs": 0}
    for client in clients:
        fixture = Path(path).parent / "clients" / client["fixture"]["name"]
        payload, digest = _read_fixture(fixture, client["fixture"]["sha256"])
        observation = _observe(payload)
        if identity is None:
            identity = _client_identity(client, fixture, digest)
        examined.append(
            {
                "fixture": f"{BASELINE_PATH.rsplit('/', 1)[0]}/clients/{fixture.name}",
                "refs_observed": observation["refs_observed"],
                "defs_observed": observation["defs_observed"],
                "unresolved_refs": observation["unresolved_refs"],
            }
        )
        for key, value in observation.items():
            total[key] += value
    status, reason = _status(total)
    return {
        "status": status,
        "reason": reason,
        "receipt_id": receipt_id(identity, status),
        "client": identity,
        "refs_observed": total["refs_observed"],
        "defs_observed": total["defs_observed"],
        "unresolved_refs": total["unresolved_refs"],
        "fixtures_examined": examined,
    }


def render(receipt) -> str:
    """Return ``receipt`` as the JSON document the probe prints."""
    return json.dumps(receipt, ensure_ascii=False, indent=2) + "\n"


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this tool's stderr format."""

    def error(self, message):
        sys.stderr.write(f"pack-shape-probe: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Probe the baseline captures and print the receipt; return the exit code."""
    parser = _Parser(
        prog="probe_pack_shape.py",
        description=(
            "Report whether the baseline-captured client resolves shared "
            "schema references."
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the receipt as JSON; the default prints one line per field",
    )
    parser.add_argument(
        "--baseline",
        metavar="PATH",
        default=None,
        help="baseline capture record to read; defaults to the frozen record",
    )
    args = parser.parse_args(argv)

    try:
        result = probe(args.baseline or BASELINE_FILE)
    except ProbeError as exc:
        sys.stderr.write(f"{exc}\n")
        return 2

    if args.json:
        sys.stdout.write(render(result))
    else:
        sys.stdout.write(
            f"status: {result['status']}\n"
            f"reason: {result['reason']}\n"
            f"receipt: {result['receipt_id']}\n"
            f"client: {result['client']['name']} {result['client']['version']} "
            f"({result['client']['provenance']})\n"
            f"fixture: {result['client']['fixture']}\n"
            f"observed: {result['refs_observed']} reference(s), "
            f"{result['defs_observed']} definition object(s), "
            f"{result['unresolved_refs']} unresolved\n"
        )
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
