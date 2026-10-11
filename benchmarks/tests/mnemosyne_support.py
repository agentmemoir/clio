"""The Mnemosyne stand-in and the fixture its fake-driven suites share.

One job: give the fake-driven Mnemosyne test classes a hermetic executable and a
scratch data directory, so `test_mnemosyne_adapter.py` holds only assertions. The
stand-in inspects ``sys.argv`` and prints the payloads the real CLI prints;
environment flags make it fail, refuse a write, or emit garbage on demand. No
test lives here.
"""

from __future__ import annotations

import os
import stat
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))

from exec_tempdir import exec_tempdir  # noqa: E402  (path set above)

from benchmarks.clio_bench.adapters.mnemosyne import MnemosyneAdapter  # noqa: E402
from benchmarks.clio_bench.tokenizer import get_tokenizer  # noqa: E402
from benchmarks.clio_bench.types import DialogueTurn  # noqa: E402

_FAKE_MNEMOSYNE = '''\
#!/usr/bin/env python3
import json
import os
import sys

BANK = os.environ.get("MNEMOSYNE_BANK", "default")
STORE = os.path.join(os.environ.get("CLIO_BENCH_STORE", ""), BANK + ".json")

if os.environ.get("CLIO_BENCH_RECORD"):
    with open(os.environ["CLIO_BENCH_RECORD"], "a", encoding="utf-8") as handle:
        handle.write(json.dumps(sys.argv[1:]) + "\\n")


def load():
    try:
        with open(STORE, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return []


def save(rows):
    os.makedirs(os.path.dirname(STORE), exist_ok=True)
    with open(STORE, "w", encoding="utf-8") as handle:
        json.dump(rows, handle)


def main():
    argv = sys.argv[1:]
    if not argv:
        sys.exit(2)
    command = argv[0]

    if os.environ.get("FAKE_MNEMOSYNE_FAIL"):
        sys.stderr.write("simulated failure\\n")
        sys.exit(3)
    if os.environ.get("FAKE_MNEMOSYNE_GARBAGE"):
        sys.stdout.write("this is not json\\n")
        return
    if command == "bank":
        return
    if command == "config":
        if len(argv) >= 3 and argv[1] == "get":
            key = argv[2]
            if os.environ.get("FAKE_MNEMOSYNE_MISSING_KEY") == key:
                sys.stdout.write(f"{key} = (not set)\\n")
            else:
                sys.stdout.write(f"{key} = 0.5\\n")
        else:
            sys.exit(2)
        return
    if command == "doctor":
        bank = argv[argv.index("--bank") + 1] if "--bank" in argv else "default"
        sys.stdout.write(
            json.dumps(
                {
                    "bank_name": bank,
                    "runtime_diagnostics": {
                        "status": "ok",
                        "checks": [
                            {
                                "check": "mnemosyne_version",
                                "detail": "9.7.5",
                                "status": "OK",
                            }
                        ],
                    },
                },
                indent=2,
            )
            + "\\n"
        )
        return
    if command == "store":
        if os.environ.get("FAKE_MNEMOSYNE_REFUSE"):
            # The real system declines a write by printing no id and exiting zero.
            sys.stdout.write("Stored: None\\n")
            return
        rows = load()
        rows.append(argv[1])
        save(rows)
        sys.stdout.write(f"Stored: mem-{len(rows)}\\n")
        return
    if command == "recall":
        rows = load()
        top_k = int(argv[2]) if len(argv) > 2 and argv[2].isdigit() else 5
        sys.stdout.write(
            json.dumps(
                {
                    "query": argv[1] if len(argv) > 1 else "",
                    "top_k": top_k,
                    "results": [
                        {"id": f"mem-{i + 1}", "content": text, "score": 1.0}
                        for i, text in enumerate(rows[:top_k])
                    ],
                }
            )
            + "\\n"
        )
        return
    sys.exit(2)


main()
'''


class MnemosyneAdapterTestCase(unittest.TestCase):
    """Shared fixtures: an executable stand-in and a scratch data directory."""

    def setUp(self) -> None:
        self._tmp = exec_tempdir(purpose="the fake mnemosyne binary")
        self.addCleanup(self._tmp.cleanup)
        self.tmpdir = self._tmp.name
        self.fake = os.path.join(self.tmpdir, "fake_mnemosyne.py")
        with open(self.fake, "w", encoding="utf-8") as handle:
            handle.write(_FAKE_MNEMOSYNE)
        os.chmod(self.fake, os.stat(self.fake).st_mode | stat.S_IXUSR)
        self.tokenizer = get_tokenizer()
        self.data_dir = os.path.join(self.tmpdir, "data")
        self.store_dir = os.path.join(self.tmpdir, "stand-in-store")
        os.makedirs(self.store_dir, exist_ok=True)

    def make_adapter(self, **kwargs) -> MnemosyneAdapter:
        kwargs.setdefault("binary", self.fake)
        kwargs.setdefault("data_dir", self.data_dir)
        kwargs.setdefault("tokenizer", self.tokenizer)
        kwargs.setdefault("extra_env", {"CLIO_BENCH_STORE": self.store_dir})
        return MnemosyneAdapter(**kwargs)

    @staticmethod
    def _turn(session_id: str = "session-a", text: str = "hello world") -> DialogueTurn:
        return DialogueTurn(
            session_id=session_id,
            turn_id="turn-1",
            speaker="user",
            role="user",
            text=text,
        )

