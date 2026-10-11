"""Hermetic tests for the Mnemosyne CLI adapter.

The stand-in executable and the fixture the fake-driven classes share live in
`mnemosyne_support.py`; this module holds the assertions. The one test that
drives the real binary lives here too, because an adapter whose whole claim is
that it can drive a real system has to be shown driving one. It skips with the
reason when the system is absent, and the skip is reported rather than passing
silently.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))

from exec_tempdir import exec_tempdir  # noqa: E402  (path set above)

from benchmarks.clio_bench.adapters.mnemosyne import (  # noqa: E402
    MnemosyneAdapter,
    MnemosyneAdapterError,
)
from benchmarks.clio_bench.fingerprint import FingerprintError  # noqa: E402
from benchmarks.clio_bench.tokenizer import get_tokenizer  # noqa: E402
from benchmarks.clio_bench.types import (  # noqa: E402
    REFUSAL_FILTERED,
    DialogueTurn,
)
from benchmarks.tests.mnemosyne_support import (  # noqa: E402
    MnemosyneAdapterTestCase,
)
from benchmarks.tests.support import assert_context_accounting  # noqa: E402

def _real_binary() -> str | None:
    """Return an installed mnemosyne, or None when this host has none.

    `CLIO_BENCH_MNEMOSYNE_BINARY` names one directly. Otherwise the search is a
    documented installation layout rather than a path baked into this file: the
    executable on PATH, or inside an agent host's own virtual environment. No
    host-specific location is committed here.
    """
    explicit = os.environ.get("CLIO_BENCH_MNEMOSYNE_BINARY")
    if explicit:
        return explicit if os.path.exists(explicit) else None
    on_path = shutil.which("mnemosyne")
    if on_path:
        return on_path
    for home_var in ("HERMES_HOME",):
        home = os.environ.get(home_var)
        if not home:
            continue
        candidate = os.path.join(home, "hermes-agent", "venv", "bin", "mnemosyne")
        if os.path.exists(candidate):
            return candidate
    return None

_LINE_1 = "[mem-1] alpha beta gamma"
_LINE_2 = "[mem-2] delta epsilon"


class HealthcheckTests(MnemosyneAdapterTestCase):
    def test_healthcheck_true_when_the_binary_answers(self) -> None:
        self.assertTrue(self.make_adapter().healthcheck())

    def test_healthcheck_false_when_the_binary_is_missing(self) -> None:
        adapter = self.make_adapter(binary="/nonexistent/mnemosyne")
        self.assertFalse(adapter.healthcheck())

    def test_healthcheck_false_when_the_binary_fails(self) -> None:
        adapter = self.make_adapter(extra_env={"FAKE_MNEMOSYNE_FAIL": "1"})
        self.assertFalse(adapter.healthcheck())


class IngestTests(MnemosyneAdapterTestCase):
    def test_ingest_records_success_and_counts_tokens(self) -> None:
        adapter = self.make_adapter()
        result = adapter.ingest("session-a", self._turn(text="hello world"))
        self.assertTrue(result.ok)
        self.assertIsNone(result.error)
        self.assertGreaterEqual(result.latency_ms, 0.0)
        self.assertEqual(result.tokens_processed, self.tokenizer.count("hello world"))

    def test_ingest_writes_into_the_session_bank(self) -> None:
        adapter = self.make_adapter()
        adapter.ingest("alpha", self._turn("alpha"))
        adapter.ingest("beta", self._turn("beta"))
        banks = sorted(os.listdir(self.store_dir))
        self.assertEqual(banks, ["clio-bench-alpha.json", "clio-bench-beta.json"])

    def test_a_declined_write_is_ok_false_not_a_crash(self) -> None:
        adapter = self.make_adapter(extra_env={"CLIO_BENCH_STORE": self.store_dir, "FAKE_MNEMOSYNE_REFUSE": "1"})
        result = adapter.ingest("session-a", self._turn())
        self.assertFalse(result.ok)
        self.assertIsNotNone(result.error)

    def test_a_declined_write_is_recorded_as_a_filtered_turn(self) -> None:
        """The system's own write filter is not an admission gate.

        Mnemosyne declines by returning no id from its write classifier, which
        drops a noise pattern or a secret; naming that an admission refusal
        would relabel filtering as a gate decision.
        """
        adapter = self.make_adapter(extra_env={"CLIO_BENCH_STORE": self.store_dir, "FAKE_MNEMOSYNE_REFUSE": "1"})
        result = adapter.ingest("session-a", self._turn())
        self.assertEqual(result.refusal, REFUSAL_FILTERED)

    def test_ingest_sends_the_pinned_importance(self) -> None:
        record = os.path.join(self.tmpdir, "argv.jsonl")
        adapter = self.make_adapter(
            importance=0.25,
            source="clio-bench",
            extra_env={
                "CLIO_BENCH_STORE": self.store_dir,
                "CLIO_BENCH_RECORD": record,
            },
        )
        adapter.ingest("session-a", self._turn(text="recorded turn"))
        with open(record, encoding="utf-8") as handle:
            calls = [json.loads(line) for line in handle if line.strip()]
        self.assertIn("0.25", calls[0])
        self.assertIn("clio-bench", calls[0])


class RetrieveTests(MnemosyneAdapterTestCase):
    def _two_turns(self) -> MnemosyneAdapter:
        adapter = self.make_adapter()
        adapter.ingest("session-a", self._turn(text="alpha beta gamma"))
        adapter.ingest("session-a", self._turn("turn-2", text="delta epsilon"))
        return adapter

    def test_retrieve_parses_results_and_respects_the_budget(self) -> None:
        adapter = self._two_turns()
        budget = self.tokenizer.count(_LINE_1)
        result = adapter.retrieve("session-a", "alpha", budget)
        self.assertEqual(result.context, _LINE_1)
        self.assertEqual(result.item_ids, ("mem-1",))
        self.assertEqual(result.tokens_retrieved, budget)
        assert_context_accounting(self, result, self.tokenizer)
        self.assertLessEqual(self.tokenizer.count(result.context), budget)

    def test_retrieve_includes_every_hit_when_the_budget_allows(self) -> None:
        adapter = self._two_turns()
        # The budget is the measured cost of the two-line context, not a sum of
        # the two line costs: the joined string is what the harness hands over,
        # so the separator it holds is part of what the budget has to cover. The
        # two tokenizers disagree about what a newline costs, so it is measured
        # through the tokenizer rather than assumed.
        budget = self.tokenizer.count(f"{_LINE_1}\n{_LINE_2}")
        result = adapter.retrieve("session-a", "alpha", budget)
        self.assertEqual(result.context, f"{_LINE_1}\n{_LINE_2}")
        self.assertEqual(result.item_ids, ("mem-1", "mem-2"))
        assert_context_accounting(self, result, self.tokenizer)
        self.assertLessEqual(self.tokenizer.count(result.context), budget)

    def test_a_budget_too_small_for_one_line_packs_nothing(self) -> None:
        # The line the system returned is the whole cost of the answer, so a
        # budget under it must yield no context at all rather than an
        # overshooting one.
        adapter = self.make_adapter()
        adapter.ingest("session-a", self._turn(text="alpha beta gamma"))
        affordable = self.tokenizer.count(_LINE_1)
        self.assertGreater(affordable, 1)

        result = adapter.retrieve("session-a", "alpha", affordable - 1)

        self.assertEqual(result.context, "")
        self.assertEqual(result.item_ids, ())
        self.assertLessEqual(self.tokenizer.count(result.context), affordable - 1)

    def test_an_over_budget_hit_stops_the_pack_rather_than_skipping_it(self) -> None:
        # Three hits where the middle one alone does not fit in what is left.
        # Packing must stop there: a later, smaller hit must not be allowed to
        # jump the queue and take budget the first two would have had, because
        # the composed context would then rank below what the system returned.
        adapter = self.make_adapter()
        adapter.ingest("session-a", self._turn(text="alpha beta gamma"))
        adapter.ingest(
            "session-a",
            self._turn("turn-2", text="a much longer piece of retrieved context text here"),
        )
        adapter.ingest("session-a", self._turn("turn-3", text="delta epsilon"))

        first = self.tokenizer.count("[mem-1] alpha beta gamma")
        third = self.tokenizer.count("[mem-3] delta epsilon")
        result = adapter.retrieve("session-a", "alpha", first + third)

        self.assertEqual(result.item_ids, ("mem-1",))
        self.assertEqual(result.context, "[mem-1] alpha beta gamma")
        self.assertLessEqual(self.tokenizer.count(result.context), first + third)

    def test_an_empty_bank_retrieves_nothing_without_failing(self) -> None:
        adapter = self.make_adapter()
        result = adapter.retrieve("session-a", "nothing stored", 4096)
        self.assertEqual(result.context, "")
        self.assertEqual(result.tokens_retrieved, 0)
        self.assertEqual(result.item_ids, ())

    def test_a_malformed_payload_raises_a_named_error(self) -> None:
        adapter = self.make_adapter(extra_env={"FAKE_MNEMOSYNE_GARBAGE": "1"})
        with self.assertRaises(MnemosyneAdapterError):
            adapter.retrieve("session-a", "alpha", 4096)

    def test_a_non_zero_exit_raises(self) -> None:
        adapter = self.make_adapter(
            extra_env={"CLIO_BENCH_STORE": self.store_dir, "FAKE_MNEMOSYNE_FAIL": "1"}
        )
        with self.assertRaises(MnemosyneAdapterError):
            adapter.retrieve("session-a", "alpha", 4096)


class ResetTests(MnemosyneAdapterTestCase):
    @staticmethod
    def _seed_bank(session_id: str, data_dir: str) -> str:
        """Create the bank directory and store a write would have created.

        The stand-in keeps its own state elsewhere, so the bank tree the adapter
        clears has to be laid down here. Without this the deletion assertions
        below would pass against a directory that never existed, which is the
        whole thing they exist to rule out.
        """
        path = os.path.join(data_dir, "banks", f"clio-bench-{session_id}")
        os.makedirs(path)
        with open(os.path.join(path, "mnemosyne.db"), "w", encoding="utf-8") as handle:
            handle.write(session_id)
        return path

    def test_reset_drops_the_session_bank_and_spares_the_others(self) -> None:
        alpha = self._seed_bank("alpha", self.data_dir)
        beta = self._seed_bank("beta", self.data_dir)

        self.make_adapter().reset("alpha")

        self.assertFalse(os.path.exists(alpha))
        self.assertTrue(os.path.isdir(beta))
        self.assertTrue(os.path.exists(os.path.join(beta, "mnemosyne.db")))
        self.assertTrue(os.path.isdir(os.path.join(self.data_dir, "banks")))

    def test_reset_creates_a_missing_data_directory(self) -> None:
        fresh = os.path.join(self.tmpdir, "fresh")
        self.make_adapter(data_dir=fresh).reset("session-a")
        self.assertTrue(os.path.isdir(os.path.join(fresh, "banks")))

    def test_reset_without_a_data_directory_deletes_nothing(self) -> None:
        # With no pinned store there is nothing of the adapter's own to clear,
        # and it must not reach outside what it was handed: a real bank tree
        # elsewhere has to survive the call.
        elsewhere = self._seed_bank("session-a", self.data_dir)

        self.make_adapter(data_dir=None).reset("session-a")

        self.assertTrue(os.path.isdir(elsewhere))
        self.assertTrue(os.path.exists(os.path.join(elsewhere, "mnemosyne.db")))


class IsolationTests(MnemosyneAdapterTestCase):
    def test_one_session_cannot_read_another_sessions_bank(self) -> None:
        adapter = self.make_adapter()
        adapter.ingest("alpha", self._turn("alpha", text="only in alpha"))
        adapter.ingest("beta", self._turn("beta", text="only in beta"))

        self.assertIn("alpha", adapter.retrieve("alpha", "alpha", 4096).context)
        self.assertNotIn("alpha", adapter.retrieve("beta", "alpha", 4096).context)

    def test_the_data_directory_and_bank_are_always_set(self) -> None:
        adapter = self.make_adapter()
        env = adapter._env("bank-a")
        self.assertEqual(env["MNEMOSYNE_DATA_DIR"], self.data_dir)
        self.assertEqual(env["MNEMOSYNE_BANK"], "bank-a")

    def test_an_ambient_bank_is_replaced_not_inherited(self) -> None:
        os.environ["MNEMOSYNE_BANK"] = "operator-bank"
        self.addCleanup(os.environ.pop, "MNEMOSYNE_BANK", None)
        adapter = self.make_adapter()
        env = adapter._env("clio-bench-s1")
        self.assertEqual(env["MNEMOSYNE_BANK"], "clio-bench-s1")
        self.assertNotIn("operator-bank", env.values())

    def test_an_ambient_data_directory_is_replaced_not_inherited(self) -> None:
        os.environ["MNEMOSYNE_DATA_DIR"] = "/somewhere/else"
        self.addCleanup(os.environ.pop, "MNEMOSYNE_DATA_DIR", None)
        adapter = self.make_adapter()
        self.assertEqual(adapter._env("bank-a")["MNEMOSYNE_DATA_DIR"], self.data_dir)

    def test_extra_env_reaches_the_subprocess(self) -> None:
        adapter = self.make_adapter(
            extra_env={"CLIO_BENCH_STORE": self.store_dir, "MNEMOSYNE_VEC_WEIGHT": "0.5"}
        )
        self.assertEqual(adapter._env("bank-a")["MNEMOSYNE_VEC_WEIGHT"], "0.5")


class FingerprintTests(MnemosyneAdapterTestCase):
    def test_fingerprint_reads_all_settings_and_the_version(self) -> None:
        payload = self.make_adapter().fingerprint().to_json()
        self.assertEqual(payload["system"], "mnemosyne")
        self.assertEqual(payload["version"], "9.7.5")
        self.assertEqual(len(payload["settings"]), 14)
        self.assertEqual(payload["settings"]["vec_weight"]["value"], 0.5)
        self.assertEqual(payload["settings"]["vec_weight"]["status"], "read")
        for key in ("cross_session", "embedding_dim", "vec_type"):
            self.assertIn(key, payload["settings"])

    def test_a_key_with_no_effective_value_raises(self) -> None:
        adapter = self.make_adapter(
            extra_env={
                "CLIO_BENCH_STORE": self.store_dir,
                "FAKE_MNEMOSYNE_MISSING_KEY": "vec_weight",
            }
        )
        with self.assertRaises(FingerprintError):
            adapter.fingerprint()

    def test_a_failing_config_read_raises(self) -> None:
        adapter = self.make_adapter(
            extra_env={"CLIO_BENCH_STORE": self.store_dir, "FAKE_MNEMOSYNE_FAIL": "1"}
        )
        with self.assertRaises(FingerprintError):
            adapter.fingerprint()

    def test_python_literals_keep_their_type(self) -> None:
        self.assertIs(MnemosyneAdapter._parse_value("True"), True)
        self.assertIs(MnemosyneAdapter._parse_value("False"), False)
        self.assertIsNone(MnemosyneAdapter._parse_value("None"))
        self.assertEqual(MnemosyneAdapter._parse_value("0.5"), 0.5)
        self.assertEqual(MnemosyneAdapter._parse_value("int8"), "int8")

    def test_the_version_probe_creates_a_scratch_bank_and_reads_doctor(self) -> None:
        record = os.path.join(self.tmpdir, "calls.jsonl")
        adapter = self.make_adapter(
            extra_env={"CLIO_BENCH_STORE": self.store_dir, "CLIO_BENCH_RECORD": record}
        )
        adapter.fingerprint()
        with open(record, encoding="utf-8") as handle:
            calls = [json.loads(line) for line in handle if line.strip()]
        self.assertIn(["bank", "create", "clio-bench-fingerprint"], calls)
        self.assertIn(
            ["doctor", "--bank", "clio-bench-fingerprint", "--format", "json"], calls
        )
        self.assertIn(["config", "get", "vec_weight"], calls)


class RealSystemTests(unittest.TestCase):
    """Drive the installed system, or skip with the reason it is absent."""

    def _installed(self) -> str:
        binary = _real_binary()
        if binary is None:
            self.skipTest(
                "no mnemosyne executable was found: not on PATH, not named by "
                "CLIO_BENCH_MNEMOSYNE_BINARY, and none under HERMES_HOME. The "
                "system may still be installed somewhere this search does not "
                "look; name it with CLIO_BENCH_MNEMOSYNE_BINARY to run the live "
                "round trip."
            )
        return binary

    def _live_adapter(self, **kwargs) -> MnemosyneAdapter:
        """Build an adapter over the installed system and a scratch data dir."""
        binary = self._installed()
        tmpdir = exec_tempdir(purpose="the mnemosyne data directory").name
        self.addCleanup(shutil.rmtree, tmpdir, True)
        adapter = MnemosyneAdapter(
            binary=binary,
            data_dir=os.path.join(tmpdir, "data"),
            tokenizer=get_tokenizer(),
            **kwargs,
        )
        if not adapter.healthcheck():
            self.skipTest(f"{binary} is present but reported itself not ready")
        return adapter

    def test_round_trip_against_the_real_system(self) -> None:
        adapter = self._live_adapter()

        adapter.reset("live")
        self.assertTrue(adapter.healthcheck())

        ingested = adapter.ingest(
            "live",
            DialogueTurn(
                session_id="live",
                turn_id="t1",
                speaker="user",
                role="user",
                text="The ambergine falcon nests on the north cliff in May.",
            ),
        )
        self.assertTrue(ingested.ok, ingested.error)
        self.assertGreaterEqual(ingested.tokens_processed, 1)

        result = adapter.retrieve("live", "Where does the falcon nest?", 4096)
        self.assertLessEqual(result.tokens_retrieved, 4096)
        self.assertTrue(result.context, "the real system returned no context for a stored turn")
        self.assertIn("falcon", result.context)

        other = adapter.retrieve("other-session", "falcon", 4096)
        self.assertEqual(other.context, "", "one session read another session's bank")

        fingerprint = adapter.fingerprint()
        fingerprint.validate()
        payload = fingerprint.to_json()
        self.assertEqual(payload["system"], "mnemosyne")
        self.assertTrue(payload["version"])
        self.assertEqual(
            payload["settings"]["embedding_model"]["status"], "read"
        )

    def test_a_real_declined_write_arrives_as_ok_false(self) -> None:
        """Only the real system can decline a write for real.

        It reports a declined write by printing no id and exiting zero, so an
        adapter that trusted the exit status would call that a success. Its own
        write filter is the only thing that declines, so this drives that filter
        through the system's documented environment variable instead of a
        stand-in. The allowed write afterwards proves the refusal came from the
        filter and not from a setup that stores nothing at all.
        """
        adapter = self._live_adapter(
            extra_env={"MNEMOSYNE_IGNORE_PATTERNS": "clio-bench-probe-phrase"}
        )
        adapter.reset("refusal")

        def turn(turn_id: str, text: str) -> DialogueTurn:
            return DialogueTurn(
                session_id="refusal",
                turn_id=turn_id,
                speaker="user",
                role="user",
                text=text,
            )

        declined = adapter.ingest(
            "refusal", turn("t1", "the clio-bench-probe-phrase should not be kept")
        )
        self.assertFalse(declined.ok)
        self.assertIsNotNone(declined.error)

        allowed = adapter.ingest("refusal", turn("t2", "The falcon nests in May."))
        self.assertTrue(allowed.ok, allowed.error)


if __name__ == "__main__":
    unittest.main()