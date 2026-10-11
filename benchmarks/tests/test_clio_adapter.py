"""Hermetic unit tests for the Clio CLI adapter.

A tiny fake ``clio`` executable is written into the test's temp directory so
no real binary, database, or network is required. The fake inspects
``sys.argv`` and prints canned JSON for ``health``, ``remember``, and
``recall``; environment flags make it fail or emit garbage on demand.
"""

from __future__ import annotations

import json
import os
import stat
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))

from exec_tempdir import exec_tempdir  # noqa: E402  (path set above)

from benchmarks.clio_bench.adapters.clio import ClioAdapter, ClioAdapterError
from benchmarks.clio_bench.fingerprint import FingerprintError
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.clio_bench.types import DialogueTurn
from benchmarks.tests.support import assert_context_accounting

_FAKE_CLIO = '''\
#!/usr/bin/env python3
import json
import os
import sys


def emit(payload):
    sys.stdout.write(json.dumps(payload) + "\\n")


def main():
    record = os.environ.get("FAKE_CLIO_RECORD")
    if record:
        with open(record, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(sys.argv[1:]) + "\\n")

    if os.environ.get("FAKE_CLIO_FAIL"):
        sys.stderr.write("simulated failure\\n")
        sys.exit(3)
    if os.environ.get("FAKE_CLIO_REJECT"):
        sys.stderr.write(
            json.dumps(
                {"ok": False, "code": "admission_refused", "message": "score below theta"}
            )
            + "\\n"
        )
        sys.exit(1)
    if os.environ.get("FAKE_CLIO_GARBAGE"):
        sys.stdout.write("this is not json\\n")
        return

    argv = sys.argv[1:]
    if not argv:
        sys.exit(2)
    command = argv[0]
    if command == "health":
        emit({"ok": True, "version": "fake"})
    elif command == "remember":
        emit({"ok": True, "item_id": "item-1"})
    elif command == "recall":
        emit(
            {
                "ok": True,
                "hits": [
                    {"item_id": "item-1", "gist": "alpha beta gamma", "snapshot": "s1"},
                    {"item_id": "item-2", "gist": "delta epsilon", "snapshot": "s2"},
                ],
            }
        )
    elif command == "ranking":
        emit({"ok": True, "retrieval": {"dense": 0.25}})
    elif command == "maintenance":
        emit({"ok": True, "index": {"pending_jobs": 0}})
    elif command == "retention":
        bank = "default"
        for index, value in enumerate(argv):
            if value == "--bank":
                bank = argv[index + 1]
        sys.stdout.write(
            json.dumps({"bank": bank, "source": "builtin", "version": 2}, indent=2)
            + "\\n"
        )
    elif command == "config":
        sys.stderr.write(
            json.dumps(
                {
                    "ok": False,
                    "code": "unknown_path",
                    "message": "unknown config path `engine_profile`",
                }
            )
            + "\\n"
        )
        sys.exit(1)
    else:
        sys.exit(2)


main()
'''

_LINE_1 = "[item-1] alpha beta gamma"
_LINE_2 = "[item-2] delta epsilon"


def _exec_tempdir():
    """Create a temp directory whose scripts can be executed.

    The rule itself lives in `scripts/exec_tempdir.py`, which every suite here and in
    `scripts/` now shares. This copy was the third; a fourth would have been a
    fourth place to update when the list of candidate directories moves.
    """
    return exec_tempdir(purpose="the fake clio binary")


class ClioAdapterTestCase(unittest.TestCase):
    """Shared fixtures: a temp dir and an executable fake clio."""

    def setUp(self) -> None:
        self._tmp = _exec_tempdir()
        self.addCleanup(self._tmp.cleanup)
        self.tmpdir = self._tmp.name
        self.fake = os.path.join(self.tmpdir, "fake_clio.py")
        with open(self.fake, "w", encoding="utf-8") as handle:
            handle.write(_FAKE_CLIO)
        os.chmod(self.fake, os.stat(self.fake).st_mode | stat.S_IXUSR)
        self.tokenizer = get_tokenizer()

    def make_adapter(self, **kwargs) -> ClioAdapter:
        kwargs.setdefault("binary", self.fake)
        kwargs.setdefault("tokenizer", self.tokenizer)
        return ClioAdapter(**kwargs)

    @staticmethod
    def _turn(session_id: str = "session-a", text: str = "hello world") -> DialogueTurn:
        return DialogueTurn(
            session_id=session_id,
            turn_id="turn-1",
            speaker="user",
            role="user",
            text=text,
        )


class HealthcheckTests(ClioAdapterTestCase):
    def test_healthcheck_true_on_ok_json(self) -> None:
        self.assertTrue(self.make_adapter().healthcheck())

    def test_healthcheck_false_on_failing_script(self) -> None:
        adapter = self.make_adapter(extra_env={"FAKE_CLIO_FAIL": "1"})
        self.assertFalse(adapter.healthcheck())


class IngestTests(ClioAdapterTestCase):
    def test_ingest_parses_ok_and_counts_tokens(self) -> None:
        adapter = self.make_adapter()
        result = adapter.ingest("session-a", self._turn(text="hello world"))
        self.assertTrue(result.ok)
        self.assertIsNone(result.error)
        self.assertGreaterEqual(result.latency_ms, 0.0)
        self.assertEqual(result.tokens_processed, self.tokenizer.count("hello world"))

    def test_ingest_uses_session_bank(self) -> None:
        record = os.path.join(self.tmpdir, "calls.jsonl")
        adapter = self.make_adapter(extra_env={"FAKE_CLIO_RECORD": record})
        adapter.ingest("alpha", self._turn("alpha"))
        adapter.ingest("beta", self._turn("beta"))
        with open(record, encoding="utf-8") as handle:
            calls = [json.loads(line) for line in handle if line.strip()]
        banks = [call[call.index("--bank") + 1] for call in calls]
        self.assertEqual(banks, ["clio-bench-alpha", "clio-bench-beta"])

    def test_ingest_records_admission_rejection_without_raising(self) -> None:
        adapter = self.make_adapter(extra_env={"FAKE_CLIO_REJECT": "1"})
        result = adapter.ingest("session-a", self._turn())
        self.assertFalse(result.ok)
        self.assertIsNotNone(result.error)
        self.assertIn("theta", result.error)


class RetrieveTests(ClioAdapterTestCase):
    def test_retrieve_parses_hits_and_respects_budget(self) -> None:
        adapter = self.make_adapter()
        budget = self.tokenizer.count(_LINE_1)
        result = adapter.retrieve("session-a", "alpha", budget)
        self.assertEqual(result.context, _LINE_1)
        self.assertEqual(result.item_ids, ("item-1",))
        self.assertEqual(result.tokens_retrieved, budget)
        self.assertLessEqual(result.tokens_retrieved, budget)
        self.assertGreaterEqual(result.latency_ms, 0.0)

    def test_retrieve_includes_all_hits_when_budget_allows(self) -> None:
        adapter = self.make_adapter()
        # The budget is the cost of the two-line context itself. The harness
        # hands over the joined string, so the separator it holds is part of
        # what the budget has to cover; a total of the two line costs is not.
        budget = self.tokenizer.count(f"{_LINE_1}\n{_LINE_2}")
        result = adapter.retrieve("session-a", "alpha", budget)
        self.assertEqual(result.context, f"{_LINE_1}\n{_LINE_2}")
        self.assertEqual(result.item_ids, ("item-1", "item-2"))
        assert_context_accounting(self, result, self.tokenizer)
        self.assertLessEqual(self.tokenizer.count(result.context), budget)


class ResetTests(ClioAdapterTestCase):
    def test_reset_deletes_only_db_under_data_dir(self) -> None:
        data_dir = os.path.join(self.tmpdir, "data")
        os.makedirs(data_dir)
        db = os.path.join(data_dir, "clio.db")
        other = os.path.join(data_dir, "other.db")
        for path in (db, other):
            with open(path, "w", encoding="utf-8"):
                pass

        outside_dir = os.path.join(self.tmpdir, "outside")
        os.makedirs(outside_dir)
        outside_db = os.path.join(outside_dir, "clio.db")
        with open(outside_db, "w", encoding="utf-8"):
            pass

        adapter = self.make_adapter(data_dir=data_dir)
        adapter.reset("session-a")

        self.assertFalse(os.path.exists(db))
        self.assertTrue(os.path.exists(other))
        self.assertTrue(os.path.exists(outside_db))

    def test_reset_creates_missing_data_dir(self) -> None:
        data_dir = os.path.join(self.tmpdir, "fresh")
        adapter = self.make_adapter(data_dir=data_dir)
        adapter.reset("session-a")
        self.assertTrue(os.path.isdir(data_dir))


class ErrorTests(ClioAdapterTestCase):
    def test_non_zero_exit_raises(self) -> None:
        adapter = self.make_adapter(extra_env={"FAKE_CLIO_FAIL": "1"})
        with self.assertRaises(ClioAdapterError):
            adapter.ingest("session-a", self._turn())

    def test_unparseable_output_raises(self) -> None:
        adapter = self.make_adapter(extra_env={"FAKE_CLIO_GARBAGE": "1"})
        with self.assertRaises(ClioAdapterError):
            adapter.ingest("session-a", self._turn())


class FingerprintTests(ClioAdapterTestCase):
    def _settings(self) -> dict:
        data_dir = os.path.join(self.tmpdir, "data-dir")
        adapter = self.make_adapter(data_dir=data_dir)
        return adapter.fingerprint().to_json()

    def test_fingerprint_reads_version_retention_and_ranking(self) -> None:
        payload = self._settings()
        self.assertEqual(payload["system"], "clio")
        self.assertEqual(payload["version"], "fake")
        retention = payload["settings"]["retention_profile"]
        self.assertEqual(retention["status"], "read")
        self.assertEqual(retention["value"]["source"], "builtin")
        ranking = payload["settings"]["ranking_environment"]
        self.assertEqual(ranking["status"], "read")
        self.assertEqual(ranking["value"]["retrieval"]["dense"], 0.25)

    def test_fingerprint_records_the_two_fields_with_no_surface(self) -> None:
        settings = self._settings()["settings"]
        trigger = settings["trigger_state"]
        self.assertEqual(trigger["status"], "unreadable")
        self.assertTrue(trigger["tried"])
        engine = settings["engine_profile"]
        self.assertEqual(engine["status"], "unreadable")
        self.assertIn("unknown config path", engine["tried"][0])

    def test_the_retention_surface_names_no_host_path(self) -> None:
        surface = self._settings()["settings"]["retention_profile"]["surface"]
        self.assertNotIn(self.tmpdir, surface)
        self.assertIn("pinned benchmark data directory", surface)

    def test_the_retention_profile_is_read_through_the_probe_bank(self) -> None:
        """The profile is read for a bank the run resolves the same way.

        The run drives one bank per session and never stores an override for
        them, so they resolve through the deployment or builtin chain. The
        read goes through a probe bank with no stored override; the `default`
        bank could carry its own override none of the run's banks would see.
        """
        payload = self._settings()
        retention = payload["settings"]["retention_profile"]
        self.assertEqual(retention["value"]["bank"], "clio-bench-fingerprint")
        self.assertIn("--bank clio-bench-fingerprint", retention["surface"])
        self.assertNotIn("--bank default", retention["surface"])

    def test_a_required_surface_that_cannot_be_read_raises(self) -> None:
        adapter = self.make_adapter(extra_env={"FAKE_CLIO_FAIL": "1"})
        with self.assertRaises(FingerprintError):
            adapter.fingerprint()


if __name__ == "__main__":
    unittest.main()
