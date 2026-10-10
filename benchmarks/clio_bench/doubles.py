"""Deterministic doubles for offline runs and hermetic tests.

`StubJudge` and `StubGenerator` implement the same call shapes as the real
LLM judge and answer generator but produce reproducible, non-model output.
They are never used for recorded baseline scores; those come from the real
judge and generator.
"""

from __future__ import annotations

from .scoring import _normalize
from .types import JudgeScore, Probe

_ABSTENTION_MARKERS = ("not known", "don't know", "do not know", "no answer", "abstain")


class StubJudge:
    """Exact-match judge double: accuracy 1.0 on a normalized match, else 0.0."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, str]] = []

    def score(self, probe: Probe, candidate: str) -> JudgeScore:
        self.calls.append((probe.probe_id, candidate))
        if probe.expect_abstention:
            lowered = candidate.lower()
            correct = any(marker in lowered for marker in _ABSTENTION_MARKERS)
        else:
            correct = bool(probe.gold_answer) and (
                _normalize(candidate) == _normalize(probe.gold_answer)
            )
        return JudgeScore(
            accuracy=1.0 if correct else 0.0,
            temporal_correctness=True,
            hallucination_penalty=0.0,
            explanation="deterministic stub exact-match judge",
            raw={"kind": "stub"},
        )


class StubGenerator:
    """Answer-generator double that echoes the first context line."""

    def __init__(self, reply: str | None = None) -> None:
        self._reply = reply

    def answer(self, question: str, context: str) -> str:
        if self._reply is not None:
            return self._reply
        first = next((line for line in context.splitlines() if line.strip()), "")
        return first or "The answer is not known from the provided context."
