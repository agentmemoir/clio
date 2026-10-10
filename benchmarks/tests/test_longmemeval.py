"""Hermetic tests for the LongMemEval dataset adapter (stdlib unittest only)."""

from __future__ import annotations

import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from benchmarks.clio_bench.datasets import longmemeval
from benchmarks.clio_bench.types import DialogueTurn

_PINS_PATH = Path(__file__).resolve().parents[1] / "clio_bench" / "pins.json"
with _PINS_PATH.open("r", encoding="utf-8") as _handle:
    PIN = json.load(_handle)["suites"]["longmemeval"]


def _raw() -> list:
    return [
        {
            "question_id": "q-1",
            "question_type": "multi-session",
            "question": "What did they decide?",
            "answer": "Move to Berlin",
            "question_date": "2023/05/01",
            "haystack_dates": ["2023/04/01", "2023/04/02"],
            "haystack_session_ids": ["s-1", "s-2"],
            "haystack_sessions": [
                [
                    {"role": "user", "content": "I like tea", "has_answer": False},
                    {"role": "assistant", "content": "Noted", "has_answer": True},
                ],
                [
                    {"role": "user", "content": "We move to Berlin", "has_answer": True},
                ],
            ],
            "answer_session_ids": ["s-1"],
        },
        {
            "question_id": "q-2",
            "question_type": "unknown-type",
            "question": "Any preference?",
            "answer": "No",
            "question_date": "2023/05/02",
            "haystack_dates": ["2023/04/03"],
            "haystack_session_ids": ["s-3"],
            "haystack_sessions": [
                [{"role": "assistant", "content": "Sure", "has_answer": False}]
            ],
            "answer_session_ids": [],
        },
    ]


class ParseTest(unittest.TestCase):
    def test_instance_count_and_ids(self) -> None:
        data = longmemeval.parse(_raw(), PIN)
        self.assertEqual(data.suite, "longmemeval")
        self.assertIs(data.pin, PIN)
        self.assertEqual(
            [inst.instance_id for inst in data.instances], ["q-1", "q-2"]
        )

    def test_session_structure_and_ids(self) -> None:
        instance = longmemeval.parse(_raw(), PIN).instances[0]
        self.assertEqual(len(instance.sessions), 2)
        self.assertEqual(
            [turn.session_id for turn in instance.sessions[0]], ["s-1", "s-1"]
        )
        self.assertEqual([turn.session_id for turn in instance.sessions[1]], ["s-2"])

    def test_turn_fields_and_role_mapping(self) -> None:
        instance = longmemeval.parse(_raw(), PIN).instances[0]
        self.assertEqual(
            instance.sessions[0][0],
            DialogueTurn(
                session_id="s-1",
                turn_id="s-1:t000",
                speaker="user",
                role="user",
                text="I like tea",
                timestamp="2023/04/01",
            ),
        )
        second = instance.sessions[0][1]
        self.assertEqual(second.turn_id, "s-1:t001")
        self.assertEqual(second.speaker, "assistant")
        self.assertEqual(second.role, "assistant")
        self.assertEqual(second.text, "Noted")

    def test_timestamp_propagation(self) -> None:
        instance = longmemeval.parse(_raw(), PIN).instances[0]
        self.assertEqual(instance.sessions[1][0].timestamp, "2023/04/02")

    def test_probe_fields(self) -> None:
        instance = longmemeval.parse(_raw(), PIN).instances[0]
        self.assertEqual(len(instance.probes), 1)
        probe = instance.probes[0]
        self.assertEqual(probe.probe_id, "q-1")
        self.assertEqual(probe.suite, "longmemeval")
        self.assertEqual(probe.instance_id, "q-1")
        self.assertEqual(probe.question, "What did they decide?")
        self.assertEqual(probe.gold_answer, "Move to Berlin")
        self.assertEqual(probe.category, "multi-hop")
        self.assertEqual(probe.evidence_ids, ("s-1",))
        self.assertFalse(probe.expect_abstention)
        self.assertIsNone(probe.adversarial_answer)

    def test_category_mapping_and_fallback(self) -> None:
        data = longmemeval.parse(_raw(), PIN)
        self.assertEqual(data.instances[0].probes[0].category, "multi-hop")
        self.assertEqual(data.instances[1].probes[0].category, "unknown-type")


class ErrorTest(unittest.TestCase):
    def test_empty_raw(self) -> None:
        with self.assertRaises(ValueError):
            longmemeval.parse([], PIN)

    def test_non_list_raw(self) -> None:
        with self.assertRaises(ValueError):
            longmemeval.parse({"nope": 1}, PIN)

    def test_missing_question_id(self) -> None:
        raw = _raw()
        del raw[0]["question_id"]
        with self.assertRaises(ValueError):
            longmemeval.parse(raw, PIN)

    def test_missing_question(self) -> None:
        raw = _raw()
        del raw[0]["question"]
        with self.assertRaises(ValueError):
            longmemeval.parse(raw, PIN)

    def test_missing_answer(self) -> None:
        raw = _raw()
        del raw[0]["answer"]
        with self.assertRaises(ValueError):
            longmemeval.parse(raw, PIN)

    def test_missing_haystack_sessions(self) -> None:
        raw = _raw()
        del raw[0]["haystack_sessions"]
        with self.assertRaises(ValueError):
            longmemeval.parse(raw, PIN)

    def test_haystack_length_mismatch(self) -> None:
        raw = _raw()
        raw[0]["haystack_dates"] = ["2023/04/01"]
        with self.assertRaises(ValueError):
            longmemeval.parse(raw, PIN)

    def test_session_turn_missing_content(self) -> None:
        raw = _raw()
        del raw[0]["haystack_sessions"][0][0]["content"]
        with self.assertRaises(ValueError):
            longmemeval.parse(raw, PIN)


class LoadTest(unittest.TestCase):
    def test_load_reads_and_parses(self) -> None:
        raw = _raw()
        with tempfile.TemporaryDirectory() as tmp:
            cache_dir = Path(tmp)
            path = cache_dir / "longmemeval_oracle.json"
            path.write_text(json.dumps(copy.deepcopy(raw)), encoding="utf-8")
            with mock.patch.object(
                longmemeval, "ensure_dataset", return_value=path
            ) as patched:
                data = longmemeval.load(PIN, cache_dir)
            patched.assert_called_once_with(PIN, cache_dir)
        self.assertEqual(data.suite, "longmemeval")
        self.assertEqual(
            [inst.instance_id for inst in data.instances], ["q-1", "q-2"]
        )
        self.assertEqual(data.instances[0].probes[0].category, "multi-hop")


if __name__ == "__main__":
    unittest.main()
