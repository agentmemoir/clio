"""Tests for the two-pass, position-swapped LLM judge."""

from __future__ import annotations

import json
import unittest

from benchmarks.clio_bench.judges.llm_judge import (
    JUDGE_SCHEMA,
    JudgeError,
    LlmJudge,
)
from benchmarks.clio_bench.llm import StubChatClient
from benchmarks.clio_bench.scoring import geometric_mean
from benchmarks.clio_bench.types import Probe


def verdict(
    accuracy: float,
    temporal: bool,
    hallucination: float,
    explanation: str,
) -> str:
    """Serialize one judge verdict as JSON."""
    return json.dumps(
        {
            "accuracy_score": accuracy,
            "temporal_correctness": temporal,
            "hallucination_penalty": hallucination,
            "explanation": explanation,
        }
    )


def normal_probe(**overrides) -> Probe:
    """A plain (non-abstention) probe."""
    fields = dict(
        probe_id="p1",
        suite="locomo",
        instance_id="i1",
        question="Where does Alice live?",
        gold_answer="Paris",
        category="single-hop",
    )
    fields.update(overrides)
    return Probe(**fields)


def segments(content: str) -> tuple[str, str]:
    """Split a judge user message into the Response A and Response B blocks."""
    a_index = content.index("Response A:")
    b_index = content.index("Response B:")
    return content[a_index:b_index], content[b_index:]


class RecordingClient:
    """Stub client that also records the keyword arguments it receives."""

    def __init__(self, replies: list[str]) -> None:
        self._stub = StubChatClient(replies)
        self.kwargs: list[dict] = []
        self.calls = self._stub.calls

    def complete(
        self,
        messages: list[dict],
        *,
        temperature: float = 0.0,
        max_tokens: int = 512,
        json_schema: dict | None = None,
    ) -> str:
        self.kwargs.append(
            {
                "temperature": temperature,
                "max_tokens": max_tokens,
                "json_schema": json_schema,
            }
        )
        return self._stub.complete(
            messages,
            temperature=temperature,
            max_tokens=max_tokens,
            json_schema=json_schema,
        )


class NormalProbeTest(unittest.TestCase):
    def test_combines_two_passes(self) -> None:
        client = StubChatClient(
            [
                verdict(0.9, True, 0.1, "first pass"),
                verdict(0.4, False, 0.3, "second pass"),
            ]
        )
        judge = LlmJudge(client, "test-model", tier="tier1")

        score = judge.score(normal_probe(), "Alice lives in Paris.")

        self.assertAlmostEqual(score.accuracy, geometric_mean([0.9, 0.4]))
        self.assertAlmostEqual(score.accuracy, 0.6)
        self.assertFalse(score.temporal_correctness)
        self.assertAlmostEqual(score.hallucination_penalty, 0.2)
        self.assertEqual(score.explanation, "first pass | second pass")
        self.assertEqual(score.raw["model"], "test-model")
        self.assertEqual(score.raw["tier"], "tier1")
        self.assertEqual(score.raw["pass1"]["accuracy_score"], 0.9)
        self.assertEqual(score.raw["pass2"]["accuracy_score"], 0.4)

    def test_temporal_is_logical_and(self) -> None:
        client = StubChatClient(
            [
                verdict(0.8, True, 0.0, "same"),
                verdict(0.8, True, 0.0, "same"),
            ]
        )
        score = LlmJudge(client, "m").score(normal_probe(), "Paris")
        self.assertTrue(score.temporal_correctness)
        # Identical explanations are not duplicated.
        self.assertEqual(score.explanation, "same")

    def test_responses_are_swapped_between_passes(self) -> None:
        client = StubChatClient(
            [
                verdict(0.5, True, 0.0, "a"),
                verdict(0.5, True, 0.0, "b"),
            ]
        )
        candidate = "CANDIDATE_ANSWER"
        gold = "GOLD_ANSWER"
        LlmJudge(client, "m").score(normal_probe(gold_answer=gold), candidate)

        self.assertEqual(len(client.calls), 2)
        a_first, b_first = segments(client.calls[0][-1]["content"])
        self.assertIn(candidate, a_first)
        self.assertIn(gold, b_first)

        a_second, b_second = segments(client.calls[1][-1]["content"])
        self.assertIn(gold, a_second)
        self.assertIn(candidate, b_second)

    def test_uses_schema_and_zero_temperature(self) -> None:
        client = RecordingClient(
            [
                verdict(0.5, True, 0.0, "a"),
                verdict(0.5, True, 0.0, "b"),
            ]
        )
        LlmJudge(client, "m").score(normal_probe(), "Paris")
        for recorded in client.kwargs:
            self.assertEqual(recorded["temperature"], 0.0)
            self.assertIs(recorded["json_schema"], JUDGE_SCHEMA)


class AbstentionProbeTest(unittest.TestCase):
    def _abstention_probe(self) -> Probe:
        return Probe(
            probe_id="p2",
            suite="locomo",
            instance_id="i1",
            question="Where did Alice move in 2019?",
            gold_answer="Paris",
            category="adversarial-abstention",
            expect_abstention=True,
            adversarial_answer="Berlin",
        )

    def test_adversarial_answer_is_not_treated_as_gold(self) -> None:
        client = StubChatClient(
            [
                verdict(0.5, True, 0.0, "a"),
                verdict(0.5, True, 0.0, "b"),
            ]
        )
        judge = LlmJudge(client, "m")
        judge.score(self._abstention_probe(), "The answer is not known.")

        content = client.calls[0][-1]["content"]
        self.assertIn("abstain", content.lower())

        # In pass 1 the gold reference is Response B; the distractor must not be
        # presented there as the correct answer.
        _, gold_segment = segments(content)
        self.assertNotIn("Berlin", gold_segment)
        self.assertIn("not known", gold_segment.lower())

        # The distractor is still surfaced, but explicitly labelled.
        self.assertIn("Berlin", content)
        self.assertIn("distractor", content.lower())

    def test_plain_gold_answer_is_not_used_for_abstention(self) -> None:
        client = StubChatClient(
            [
                verdict(0.0, True, 0.0, "a"),
                verdict(0.0, True, 0.0, "b"),
            ]
        )
        LlmJudge(client, "m").score(self._abstention_probe(), "I don't know")
        _, gold_segment = segments(client.calls[0][-1]["content"])
        self.assertNotIn("Paris", gold_segment)


class ParseFailureTest(unittest.TestCase):
    def test_unparseable_json_raises(self) -> None:
        client = StubChatClient(["this is not json", verdict(0.5, True, 0.0, "b")])
        judge = LlmJudge(client, "m")
        with self.assertRaises(JudgeError) as ctx:
            judge.score(normal_probe(), "Paris")
        self.assertIn("this is not json", str(ctx.exception))

    def test_out_of_range_accuracy_raises(self) -> None:
        client = StubChatClient(
            [
                verdict(1.5, True, 0.0, "a"),
                verdict(0.5, True, 0.0, "b"),
            ]
        )
        with self.assertRaises(JudgeError):
            LlmJudge(client, "m").score(normal_probe(), "Paris")

    def test_missing_key_raises(self) -> None:
        incomplete = json.dumps(
            {
                "accuracy_score": 0.5,
                "temporal_correctness": True,
                "hallucination_penalty": 0.0,
            }
        )
        client = StubChatClient([incomplete, verdict(0.5, True, 0.0, "b")])
        with self.assertRaises(JudgeError):
            LlmJudge(client, "m").score(normal_probe(), "Paris")

    def test_non_boolean_temporal_raises(self) -> None:
        client = StubChatClient(
            [
                verdict(0.5, 1, 0.0, "a"),  # type: ignore[arg-type]
                verdict(0.5, True, 0.0, "b"),
            ]
        )
        with self.assertRaises(JudgeError):
            LlmJudge(client, "m").score(normal_probe(), "Paris")


class CodeFenceTest(unittest.TestCase):
    def test_json_fence_is_stripped(self) -> None:
        fenced = "```json\n" + verdict(0.5, True, 0.0, "ok") + "\n```"
        client = StubChatClient([fenced, fenced])
        score = LlmJudge(client, "m").score(normal_probe(), "Paris")
        self.assertAlmostEqual(score.accuracy, 0.5)

    def test_bare_fence_is_stripped(self) -> None:
        fenced = "```\n" + verdict(0.5, True, 0.0, "ok") + "\n```"
        client = StubChatClient([fenced, fenced])
        score = LlmJudge(client, "m").score(normal_probe(), "Paris")
        self.assertAlmostEqual(score.accuracy, 0.5)


if __name__ == "__main__":
    unittest.main()
