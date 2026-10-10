"""Shared test fixtures for the clio-bench suites.

The stand-in system and the synthetic suite live here because more than one
test module drives them. A shared fixture holds no tests of its own; it only
gives the suites one copy of the things they would otherwise re-define or
import from each other.
"""

from __future__ import annotations

from benchmarks.clio_bench.types import DialogueTurn, Instance, Probe, SuiteData

# One stand-in answers both documented CLI surfaces. Its recall payload carries
# the clio key and the mnemosyne key in the same object, and each adapter reads
# only its own, so this file holds no per-adapter knowledge about retrieval. It
# keeps one file per bank, the way a real bank isolates, so a session-isolation
# assertion tests the adapter rather than the stand-in. Its `retention` branch
# resolves the store the way the real clio does, from the data directory the
# adapter passes through the environment, so a test can prove which directory a
# capture read.
FAKE_SYSTEM = '''\
#!/usr/bin/env python3
import json
import os
import sys

STATE_DIR = os.environ.get("CLIO_BENCH_FAKE_STATE", "")


def bank_name(argv):
    for index, value in enumerate(argv):
        if value == "--bank":
            return argv[index + 1]
    return os.environ.get("MNEMOSYNE_BANK", "default")


def state_path(argv):
    return os.path.join(STATE_DIR, bank_name(argv) + ".txt")


def stored(argv):
    try:
        with open(state_path(argv), encoding="utf-8") as handle:
            return [line for line in handle.read().splitlines() if line]
    except OSError:
        return []


def remember(argv, text):
    if STATE_DIR:
        os.makedirs(STATE_DIR, exist_ok=True)
        with open(state_path(argv), "a", encoding="utf-8") as handle:
            handle.write(text + "\\n")


def emit(payload):
    sys.stdout.write(json.dumps(payload) + "\\n")


def main():
    argv = sys.argv[1:]
    if not argv:
        sys.exit(2)
    command = argv[0]

    if os.environ.get("CLIO_BENCH_FAKE_REFUSE") and command in ("store", "remember"):
        # Each system reports a refusal its own way: mnemosyne exits zero with no
        # id, clio exits non-zero with the result object on stderr.
        if command == "store":
            sys.stdout.write("Stored: None\\n")
        else:
            sys.stderr.write(
                json.dumps(
                    {"ok": False, "code": "admission_refused", "message": "below theta"}
                )
                + "\\n"
            )
            sys.exit(1)
        return

    if command == "health":
        emit({"ok": True, "version": "fake"})
    elif command == "remember":
        remember(argv, argv[1])
        emit({"ok": True, "item_id": f"item-{len(stored(argv))}"})
    elif command == "store":
        remember(argv, argv[1])
        sys.stdout.write(f"Stored: mem-{len(stored(argv))}\\n")
    elif command == "bank":
        return
    elif command == "config":
        if len(argv) >= 3 and argv[1] == "get":
            # `config get <key>` answers in the text form the text-reading
            # systems parse; a JSON-reading system gets no object and records
            # its own explicit miss, which is the shape contract this covers.
            sys.stdout.write(f"{argv[2]} = 0\\n")
        else:
            sys.exit(2)
    elif command == "doctor":
        emit(
            {
                "bank_name": "clio-bench-fingerprint",
                "runtime_diagnostics": {
                    "status": "ok",
                    "checks": [
                        {
                            "check": "mnemosyne_version",
                            "detail": "0.0.0-fake",
                            "status": "OK",
                        }
                    ],
                },
            }
        )
    elif command == "retention":
        bank = bank_name(argv)
        data_dir = os.environ.get("CLIO_DATA_DIR", "")
        store = (
            os.path.join(data_dir, "retention-profiles.json") if data_dir else ""
        )
        source = "bank" if store and os.path.exists(store) else "builtin"
        sys.stdout.write(
            json.dumps({"bank": bank, "source": source, "version": 2}, indent=2)
            + "\\n"
        )
    elif command == "ranking":
        emit({"ok": True, "retrieval": {"dense": 0.25}})
    elif command == "maintenance":
        emit({"ok": True, "index": {"pending_jobs": 0}})
    elif command == "recall":
        lines = stored(argv)[:5]
        emit(
            {
                "ok": True,
                "hits": [
                    {"item_id": f"item-{i + 1}", "gist": text, "snapshot": "s1"}
                    for i, text in enumerate(lines)
                ],
                "results": [
                    {"id": f"mem-{i + 1}", "content": text} for i, text in enumerate(lines)
                ],
            }
        )
    else:
        sys.exit(2)


main()
'''


def _probe(probe_id: str, instance_id: str, category: str) -> Probe:
    return Probe(
        probe_id=probe_id,
        suite="locomo",
        instance_id=instance_id,
        question="Where did Alice go?",
        gold_answer="Paris",
        category=category,
    )


def synthetic_suite() -> SuiteData:
    """A small in-memory suite: five instances, two categories, one session."""
    instances = []
    for index in range(5):
        instance_id = f"i{index}"
        probes = [
            _probe(f"{instance_id}-p1", instance_id, "single-hop"),
            _probe(f"{instance_id}-p2", instance_id, "temporal-reasoning"),
        ]
        instances.append(
            Instance(
                instance_id=instance_id,
                sessions=[
                    [
                        DialogueTurn(
                            session_id="s",
                            turn_id="t0",
                            speaker="user",
                            role="user",
                            text="Alice went to Paris in 2024.",
                        )
                    ]
                ],
                probes=probes,
            )
        )
    return SuiteData(suite="locomo", instances=instances, pin={"name": "synthetic"})