"""Hermetic tests for the LoCoMo dataset adapter (stdlib unittest only)."""

from __future__ import annotations

import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from benchmarks.clio_bench.datasets import locomo
from benchmarks.clio_bench.types import DialogueTurn

_PINS_PATH = Path(__file__).resolve().parents[1] / "clio_bench" / "pins.json"
with _PINS_PATH.open("r", encoding="utf-8") as _handle:
    PIN = json.load(_handle)["suites"]["locomo"]


def _raw() -> list:
    return [
        {
            "sample_id": "conv-1",
            "conversation": {
                "speaker_a": "Alice",
                "speaker_b": "Bob",
                # Session 2 listed before session 1 on purpose: ordering must
                # follow the numeric suffix, not dict insertion order.
                "session_2_date_time": "2:00 pm on 2 January, 2023",
                "session_2": [
                    {"speaker": "Bob", "dia_id": "D2:1", "text": "Remember the trip"},
                ],
                "session_1_date_time": "1:00 pm on 1 January, 2023",
                "session_1": [
                    {"speaker": "Alice", "dia_id": "D1:1", "text": "Hi Bob"},
                    {"speaker": "Bob", "dia_id": "D1:2", "text": "Hi Alice"},
                ],
            },
            "qa": [
                {
                    "question": "Where did they go?",
                    "answer": "Paris",
                    "category": 1,
                    "evidence": ["D1:1"],
                },
                {
                    "question": "When did they talk?",
                    "answer": "January",
                    "category": 2,
                    "evidence": ["D2:1"],
                },
                {
                    "question": "Which planet?",
                    "adversarial_answer": "Mars",
                    "category": 5,
                },
            ],
            "event_summary": {},
            "observation": {},
            "session_summary": {},
        },
        {
            "sample_id": "conv-2",
            "conversation": {
                "speaker_a": "Carol",
                "speaker_b": "Dave",
                "session_3_date_time": "3:00 pm on 3 March, 2023",
                "session_3": [
                    {"speaker": "Carol", "dia_id": "D3:1", "text": "Hello"},
                ],
            },
            "qa": [
                {"question": "Who?", "answer": "Carol", "category": 9, "evidence": []},
            ],
        },
    ]


class ParseTest(unittest.TestCase):
    def test_instance_count_and_ids(self) -> None:
        data = locomo.parse(_raw(), PIN)
        self.assertEqual(data.suite, "locomo")
        self.assertIs(data.pin, PIN)
        self.assertEqual(
            [inst.instance_id for inst in data.instances], ["conv-1", "conv-2"]
        )

    def test_session_ordering_and_ids(self) -> None:
        instance = locomo.parse(_raw(), PIN).instances[0]
        self.assertEqual(len(instance.sessions), 2)
        self.assertEqual(
            [turn.session_id for turn in instance.sessions[0]],
            ["conv-1:session_1", "conv-1:session_1"],
        )
        self.assertEqual(
            [turn.session_id for turn in instance.sessions[1]],
            ["conv-1:session_2"],
        )

    def test_turn_fields_and_role_mapping(self) -> None:
        instance = locomo.parse(_raw(), PIN).instances[0]
        self.assertEqual(
            instance.sessions[0][0],
            DialogueTurn(
                session_id="conv-1:session_1",
                turn_id="D1:1",
                speaker="Alice",
                role="user",
                text="Hi Bob",
                timestamp="1:00 pm on 1 January, 2023",
            ),
        )
        self.assertEqual(instance.sessions[0][1].speaker, "Bob")
        self.assertEqual(instance.sessions[0][1].role, "assistant")
        self.assertEqual(instance.sessions[0][1].turn_id, "D1:2")

    def test_timestamp_propagation(self) -> None:
        instance = locomo.parse(_raw(), PIN).instances[0]
        self.assertEqual(
            instance.sessions[1][0].timestamp, "2:00 pm on 2 January, 2023"
        )

    def test_probe_fields(self) -> None:
        instance = locomo.parse(_raw(), PIN).instances[0]
        self.assertEqual(
            [probe.probe_id for probe in instance.probes],
            ["conv-1:q000", "conv-1:q001", "conv-1:q002"],
        )
        probe = instance.probes[0]
        self.assertEqual(probe.suite, "locomo")
        self.assertEqual(probe.instance_id, "conv-1")
        self.assertEqual(probe.question, "Where did they go?")
        self.assertEqual(probe.gold_answer, "Paris")
        self.assertEqual(probe.category, "single-hop")
        self.assertEqual(probe.evidence_ids, ("D1:1",))
        self.assertFalse(probe.expect_abstention)
        self.assertIsNone(probe.adversarial_answer)

    def test_category_mapping_and_fallback(self) -> None:
        data = locomo.parse(_raw(), PIN)
        self.assertEqual(data.instances[0].probes[1].category, "temporal-reasoning")
        self.assertEqual(data.instances[1].probes[0].category, "9")

    def test_adversarial_abstention(self) -> None:
        probe = locomo.parse(_raw(), PIN).instances[0].probes[2]
        self.assertEqual(probe.category, "adversarial-abstention")
        self.assertEqual(probe.gold_answer, "")
        self.assertTrue(probe.expect_abstention)
        self.assertEqual(probe.adversarial_answer, "Mars")


class ErrorTest(unittest.TestCase):
    def test_empty_raw(self) -> None:
        with self.assertRaises(ValueError):
            locomo.parse([], PIN)

    def test_non_list_raw(self) -> None:
        with self.assertRaises(ValueError):
            locomo.parse({"nope": 1}, PIN)

    def test_missing_sample_id(self) -> None:
        raw = _raw()
        del raw[0]["sample_id"]
        with self.assertRaises(ValueError):
            locomo.parse(raw, PIN)

    def test_missing_conversation_body(self) -> None:
        raw = _raw()
        del raw[0]["conversation"]
        with self.assertRaises(ValueError):
            locomo.parse(raw, PIN)

    def test_missing_qa(self) -> None:
        raw = _raw()
        del raw[0]["qa"]
        with self.assertRaises(ValueError):
            locomo.parse(raw, PIN)

    def test_missing_speaker_a(self) -> None:
        raw = _raw()
        del raw[0]["conversation"]["speaker_a"]
        with self.assertRaises(ValueError):
            locomo.parse(raw, PIN)

    def test_qa_missing_answer(self) -> None:
        raw = _raw()
        del raw[0]["qa"][0]["answer"]
        with self.assertRaises(ValueError):
            locomo.parse(raw, PIN)

    def test_qa_missing_question(self) -> None:
        raw = _raw()
        del raw[0]["qa"][0]["question"]
        with self.assertRaises(ValueError):
            locomo.parse(raw, PIN)


class LoadTest(unittest.TestCase):
    def test_load_reads_and_parses(self) -> None:
        raw = _raw()
        with tempfile.TemporaryDirectory() as tmp:
            cache_dir = Path(tmp)
            path = cache_dir / "locomo10.json"
            path.write_text(json.dumps(copy.deepcopy(raw)), encoding="utf-8")
            with mock.patch.object(
                locomo, "ensure_dataset", return_value=path
            ) as patched:
                data = locomo.load(PIN, cache_dir)
            patched.assert_called_once_with(PIN, cache_dir)
        self.assertEqual(data.suite, "locomo")
        self.assertEqual(
            [inst.instance_id for inst in data.instances], ["conv-1", "conv-2"]
        )
        self.assertEqual(data.instances[0].probes[0].gold_answer, "Paris")


if __name__ == "__main__":
    unittest.main()
