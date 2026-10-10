"""Tests for the context-grounded answer generator."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.answerer import AnswerGenerator
from benchmarks.clio_bench.llm import StubChatClient


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


class AnswerGeneratorTest(unittest.TestCase):
    def test_returns_stripped_content(self) -> None:
        client = StubChatClient(["  Paris  "])
        generator = AnswerGenerator(client, "m")
        self.assertEqual(generator.answer("Where?", "Alice lives in Paris."), "Paris")

    def test_context_and_question_reach_the_model(self) -> None:
        client = StubChatClient(["Paris"])
        context = "Alice has lived in Paris since 2015."
        question = "Where does Alice live?"
        AnswerGenerator(client, "m").answer(question, context)

        self.assertEqual(len(client.calls), 1)
        content = client.calls[0][-1]["content"]
        self.assertIn(context, content)
        self.assertIn(question, content)

    def test_empty_model_reply_falls_back(self) -> None:
        client = StubChatClient(["   "])
        result = AnswerGenerator(client, "m").answer("Where?", "No useful context.")
        self.assertTrue(result.strip())
        self.assertIn("not known", result.lower())

    def test_single_call_with_zero_temperature(self) -> None:
        client = RecordingClient(["Paris"])
        AnswerGenerator(client, "m", max_tokens=64).answer("Where?", "Alice in Paris.")
        self.assertEqual(len(client.kwargs), 1)
        self.assertEqual(client.kwargs[0]["temperature"], 0.0)
        self.assertEqual(client.kwargs[0]["max_tokens"], 64)
        self.assertIsNone(client.kwargs[0]["json_schema"])


if __name__ == "__main__":
    unittest.main()
