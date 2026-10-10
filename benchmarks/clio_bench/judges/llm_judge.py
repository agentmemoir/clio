"""Calibrated, structured LLM-as-a-judge for clio-bench.

The judge scores one candidate answer against a reference answer using a
strict JSON rubric. To cancel positional bias, every probe is judged twice
with the two responses swapped (A/B then B/A) and the accuracy scores are
combined with a geometric mean. If either pass is not a valid verdict the
judge raises `JudgeError` rather than fabricating a score.
"""

from __future__ import annotations

import json
import re

from ..llm import ChatClient
from ..scoring import geometric_mean
from ..types import JudgeScore, Probe

JUDGE_SCHEMA: dict = {
    "type": "object",
    "properties": {
        "accuracy_score": {"type": "number", "minimum": 0, "maximum": 1},
        "temporal_correctness": {"type": "boolean"},
        "hallucination_penalty": {"type": "number", "minimum": 0, "maximum": 1},
        "explanation": {"type": "string"},
    },
    "required": [
        "accuracy_score",
        "temporal_correctness",
        "hallucination_penalty",
        "explanation",
    ],
    "additionalProperties": False,
}

_REQUIRED_KEYS = (
    "accuracy_score",
    "temporal_correctness",
    "hallucination_penalty",
    "explanation",
)

_ABSTENTION_GOLD = (
    "(the correct behavior is to abstain: state that the answer is not known)"
)

_MAX_ECHO = 200

_FENCE = re.compile(r"```(?:json)?\s*(.*?)\s*```", re.DOTALL)

_SYSTEM_PROMPT = (
    "You are a strict, calibrated judge for a long-term conversational memory "
    "benchmark. You compare one candidate answer against a reference answer and "
    "emit a structured verdict.\n"
    "Rubric:\n"
    "- accuracy_score: number in [0,1]. 1 means the candidate conveys the same "
    "information as the reference answer; 0 means it is wrong or unrelated.\n"
    "- temporal_correctness: boolean. True only when the candidate's time "
    "references agree with the reference; use true when the question has no "
    "temporal component.\n"
    "- hallucination_penalty: number in [0,1]. 0 means no invented facts; 1 "
    "means the candidate is mostly fabricated.\n"
    "- explanation: one concise sentence justifying the scores.\n"
    "Judge only the content, never the position of the response. Return only a "
    "JSON object matching the provided schema."
)


class JudgeError(RuntimeError):
    """Raised when a judge pass returns unparseable or invalid JSON."""


def _truncate(text: str, limit: int = _MAX_ECHO) -> str:
    """Shorten `text` for inclusion in an error message."""
    if len(text) <= limit:
        return text
    return text[:limit] + "..."


def _strip_code_fences(text: str) -> str:
    """Return the payload inside a ```json ... ``` fence when one is present."""
    match = _FENCE.search(text)
    if match:
        return match.group(1).strip()
    return text.strip()


def _is_unit_number(value: object) -> bool:
    """True for a real number in [0,1] (booleans are not numbers here)."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    return 0.0 <= float(value) <= 1.0


class LlmJudge:
    """Two-pass, position-swapped LLM judge producing a `JudgeScore`."""

    def __init__(
        self,
        client: ChatClient,
        model: str,
        *,
        tier: str = "tier1",
        max_tokens: int = 200,
    ) -> None:
        self.client = client
        self.model = model
        self.tier = tier
        self.max_tokens = max_tokens

    def score(self, probe: Probe, candidate: str) -> JudgeScore:
        """Score `candidate` for `probe` using two swapped judge passes."""
        gold = self._gold_text(probe)
        context = self._context_block(probe)

        pass1_raw = self.client.complete(
            self._messages(context, candidate, gold, "Response A"),
            temperature=0.0,
            max_tokens=self.max_tokens,
            json_schema=JUDGE_SCHEMA,
        )
        pass2_raw = self.client.complete(
            self._messages(context, gold, candidate, "Response B"),
            temperature=0.0,
            max_tokens=self.max_tokens,
            json_schema=JUDGE_SCHEMA,
        )

        pass1 = self._parse_pass(pass1_raw, "pass 1")
        pass2 = self._parse_pass(pass2_raw, "pass 2")

        accuracy = geometric_mean(
            [float(pass1["accuracy_score"]), float(pass2["accuracy_score"])]
        )
        temporal_correctness = bool(
            pass1["temporal_correctness"] and pass2["temporal_correctness"]
        )
        hallucination_penalty = (
            float(pass1["hallucination_penalty"])
            + float(pass2["hallucination_penalty"])
        ) / 2.0

        explanation = str(pass1["explanation"])
        second = str(pass2["explanation"])
        if second != explanation:
            explanation = f"{explanation} | {second}"

        return JudgeScore(
            accuracy=accuracy,
            temporal_correctness=temporal_correctness,
            hallucination_penalty=hallucination_penalty,
            explanation=explanation,
            raw={
                "model": self.model,
                "tier": self.tier,
                "pass1": pass1,
                "pass2": pass2,
            },
        )

    def _gold_text(self, probe: Probe) -> str:
        """Reference text shown to the judge for `probe`."""
        if probe.expect_abstention:
            return _ABSTENTION_GOLD
        return probe.gold_answer

    def _context_block(self, probe: Probe) -> str:
        """Question plus abstention/distractor notes shown before the responses."""
        parts = [f"Question:\n{probe.question}"]
        if probe.expect_abstention:
            parts.append(
                "Note: this probe expects abstention. The correct behavior is to "
                "abstain: state that the answer is not known."
            )
            if probe.adversarial_answer:
                parts.append(
                    "Distractor (a plausible but incorrect answer; do NOT treat it "
                    f"as correct):\n{probe.adversarial_answer}"
                )
        return "\n\n".join(parts) + "\n\n"

    def _messages(
        self, context: str, a_text: str, b_text: str, target: str
    ) -> list[dict]:
        """Build the message list for one pass, rating `target`."""
        user = (
            f"{context}"
            f"Response A:\n{a_text}\n\n"
            f"Response B:\n{b_text}\n\n"
            f"Rate {target} against the reference answer using the rubric. "
            "Return only the JSON object."
        )
        return [
            {"role": "system", "content": _SYSTEM_PROMPT},
            {"role": "user", "content": user},
        ]

    def _parse_pass(self, content: str, label: str) -> dict:
        """Parse and validate one pass, raising `JudgeError` on any problem."""
        text = _strip_code_fences(content)
        try:
            parsed = json.loads(text)
        except (json.JSONDecodeError, TypeError) as exc:
            raise JudgeError(
                f"{label} returned unparseable JSON: {_truncate(content)}"
            ) from exc

        if not isinstance(parsed, dict):
            raise JudgeError(
                f"{label} did not return a JSON object: {_truncate(content)}"
            )

        missing = [key for key in _REQUIRED_KEYS if key not in parsed]
        if missing:
            raise JudgeError(
                f"{label} missing keys {missing}: {_truncate(content)}"
            )

        if not _is_unit_number(parsed["accuracy_score"]):
            raise JudgeError(
                f"{label} accuracy_score must be a number in [0,1]: "
                f"{_truncate(content)}"
            )
        if not _is_unit_number(parsed["hallucination_penalty"]):
            raise JudgeError(
                f"{label} hallucination_penalty must be a number in [0,1]: "
                f"{_truncate(content)}"
            )
        if not isinstance(parsed["temporal_correctness"], bool):
            raise JudgeError(
                f"{label} temporal_correctness must be a boolean: "
                f"{_truncate(content)}"
            )
        if not isinstance(parsed["explanation"], str):
            raise JudgeError(
                f"{label} explanation must be a string: {_truncate(content)}"
            )

        return parsed
