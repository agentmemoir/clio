#!/usr/bin/env python3
"""Probe what the baseline-captured payloads show about shared schema references.

The pack ships exactly one shape, and which one it ships is decided by evidence
rather than by preference. This probe produces that evidence: it reads the
client captures the frozen baseline names and reports what those captures
actually show about local `#/...` pointers and the `$defs` objects they resolve
against.

A capture is bytes a server sent to a client. Those bytes can *refute* that the
client resolves shared schema references — a local `$ref` whose target is absent,
or a schema observably stripped of the content it should carry, shows the client
did not receive a resolvable document. They cannot *establish* support: nothing
in the bytes records whether the client followed a pointer, and the specification
mandates JSON Schema 2020-12 for schemas without `$schema`, permits local
`$ref`/`$defs`, and forbids network dereference, so a well-formed capture is also
what a client that ignores references would receive. This module therefore never
infers support from a capture:

- ``failed`` only when the capture refutes support: a local `$ref` is present
  and its target does not resolve inside the captured document, or a captured
  schema is present with no content at all (``{}`` or ``null``).
- ``not_run`` in every other case, explicitly including a capture whose local
  `$ref`s are present and all resolve. The reason says a captured payload can
  refute reference resolution but cannot establish it, so support is unobserved.
- ``passed`` stays an allowed status in the receipt format and in `choose()`, so
  a future observation path can report one, but no capture path produces it.

An unavailable capture — a missing or unreadable fixture, a capture that is not
JSON, or one whose digest does not match the digest the baseline recorded — is
also ``not_run``. The receipt names what was unavailable, carries whatever client
identity the baseline record states, and is still printed and recorded, so "if
unavailable, record Not run and ship the flattened candidate" covers that case
too. A run that honestly records ``not_run`` is a successful run and exits 0;
only a usage error exits 2.

Nothing here runs a client, reaches the network, or reads the pack, so two runs
on one checkout agree; the id derives from the fixture digest and status.
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

#: Why no capture can ever establish support; every non-failure receipt says it.
REFUTE_ONLY = (
    "a captured payload can refute reference resolution but cannot establish it, "
    "so client support for shared schema references is unobserved"
)

#: The members of a captured tool definition that carry a schema document.
SCHEMA_MEMBERS = ("inputSchema", "outputSchema")

#: The receipt-id token used when not even a recorded fixture digest is known.
UNKNOWN_CLIENT = "unknown-client"

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

    A local reference resolves against the payload and every enclosing object
    that carries a `$defs`, the way a client reads it.
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


def _stripped(payload) -> int:
    """Return how many captured schemas carry no content at all.

    A schema member that is ``null`` or ``{}`` holds none of the keys a schema
    should carry, so the capture is observably stripped of content.
    """
    count = 0
    stack = [payload]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            for key, value in node.items():
                if key in SCHEMA_MEMBERS and (value is None or value == {}):
                    count += 1
                stack.append(value)
        elif isinstance(node, list):
            stack.extend(node)
    return count


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
        _fail(f"capture {path.name} is unavailable: {exc}")
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
        _fail(f"the baseline record is unavailable: {exc}")
    try:
        record = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        _fail(f"the baseline record is not valid UTF-8: {exc}")
    except json.JSONDecodeError as exc:
        _fail(f"the baseline record is not valid JSON: {exc}")
    if not isinstance(record, dict):
        _fail(f"the baseline record must be an object, got {type(record).__name__}")
    return record


def _status(observation, stripped=0) -> tuple:
    """Return the status and its plain-words reason for one observation.

    Only a refutation produces ``failed``. A capture that is merely silent, or
    whose local references all resolve, is ``not_run``, never ``passed``, because
    a capture cannot establish that a client follows a pointer.
    """
    if stripped:
        return (
            STATUS_FAILED,
            f"the baseline-captured payload holds {stripped} schema(s) with no "
            "content at all, so the capture is observably stripped of content it "
            "should carry and cannot carry shared schema references",
        )
    refs = observation["refs_observed"]
    if observation["unresolved_refs"]:
        return (
            STATUS_FAILED,
            f"the baseline-captured payload holds {refs} local reference(s) and "
            f"{observation['unresolved_refs']} of them do not resolve inside the "
            "captured document",
        )
    if refs == 0:
        observed_defs = observation["defs_observed"]
        extra = (
            f" and {observed_defs} local definition object(s)"
            if observed_defs
            else ""
        )
        detail = "the baseline-captured payload holds no shared schema reference" + extra
    else:
        detail = (
            f"the baseline-captured payload holds {refs} local reference(s) and "
            "every one resolves inside the captured document"
        )
    return (STATUS_NOT_RUN, f"{detail}; {REFUTE_ONLY}")


def receipt_id(client, status) -> str:
    """Return the stable receipt id derived from a fixture digest and a status.

    No clock value and no environment value enters this id, so the same capture
    and the same status always produce the same receipt.
    """
    digest = client.get("sha256") if isinstance(client, dict) else None
    token = digest[:16] if isinstance(digest, str) and digest else UNKNOWN_CLIENT
    return f"pack-shape-probe-{token}-{status}"


def _unavailable(detail, client=None) -> dict:
    """Return a ``not_run`` receipt for a capture the probe could not read.

    ``client`` carries whatever identity the baseline stated, including the
    digest it expected, so a reader sees which capture went missing.
    """
    return {
        "status": STATUS_NOT_RUN,
        "reason": f"{detail}; {REFUTE_ONLY}",
        "receipt_id": receipt_id(client, STATUS_NOT_RUN),
        "client": client,
        "capture_available": False,
        "refs_observed": 0,
        "defs_observed": 0,
        "unresolved_refs": 0,
        "stripped_schemas": 0,
        "fixtures_examined": [],
    }


def probe(path=BASELINE_FILE) -> dict:
    """Return the receipt for the client captures the baseline record names.

    An unavailable baseline record or capture never raises: the receipt is
    ``not_run`` naming what was unavailable, so the record can still be written.
    """
    try:
        record = load_baseline(path)
        clients = _clients(record)
    except ProbeError as exc:
        return _unavailable(str(exc))

    examined = []
    identity = None
    total = {
        "refs_observed": 0,
        "defs_observed": 0,
        "unresolved_refs": 0,
        "stripped_schemas": 0,
    }
    for client in clients:
        fixture = Path(path).parent / "clients" / client["fixture"]["name"]
        stated = _client_identity(client, fixture, client["fixture"]["sha256"])
        try:
            payload, digest = _read_fixture(fixture, client["fixture"]["sha256"])
        except ProbeError as exc:
            return _unavailable(str(exc), stated)
        observation = _observe(payload)
        stripped = _stripped(payload)
        if identity is None:
            identity = _client_identity(client, fixture, digest)
        examined.append(
            {
                "fixture": f"{BASELINE_PATH.rsplit('/', 1)[0]}/clients/{fixture.name}",
                "refs_observed": observation["refs_observed"],
                "defs_observed": observation["defs_observed"],
                "unresolved_refs": observation["unresolved_refs"],
                "stripped_schemas": stripped,
            }
        )
        for key, value in observation.items():
            total[key] += value
        total["stripped_schemas"] += stripped

    status, reason = _status(total, total["stripped_schemas"])
    return {
        "status": status,
        "reason": reason,
        "receipt_id": receipt_id(identity, status),
        "client": identity,
        "capture_available": True,
        "refs_observed": total["refs_observed"],
        "defs_observed": total["defs_observed"],
        "unresolved_refs": total["unresolved_refs"],
        "stripped_schemas": total["stripped_schemas"],
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
    """Probe the baseline captures and print the receipt; return the exit code.

    A receipt with status ``not_run`` is an honest observation and exits 0; only
    a usage error exits 2, so an unavailable capture still yields a receipt.
    """
    parser = _Parser(
        prog="probe_pack_shape.py",
        description="Report what the baseline captures show about shared references.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print only the JSON receipt, so stdout can be piped",
    )
    parser.add_argument(
        "--baseline",
        metavar="PATH",
        default=None,
        help="baseline capture record the probe reads",
    )
    args = parser.parse_args(argv)

    result = probe(args.baseline or BASELINE_FILE)

    if args.json:
        sys.stdout.write(render(result))
    else:
        client = result["client"] or {}
        sys.stdout.write(
            f"status: {result['status']}\n"
            f"reason: {result['reason']}\n"
            f"receipt: {result['receipt_id']}\n"
            f"client: {client.get('name', 'unknown')} "
            f"{client.get('version', 'unknown')} "
            f"({client.get('provenance', 'unknown')})\n"
            f"fixture: {client.get('fixture', 'unknown')}\n"
            f"capture: {'available' if result['capture_available'] else 'unavailable'}\n"
            f"observed: {result['refs_observed']} reference(s), "
            f"{result['defs_observed']} definition object(s), "
            f"{result['unresolved_refs']} unresolved, "
            f"{result['stripped_schemas']} stripped schema(s)\n"
        )
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
