"""LoCoMo suite adapter for clio-bench.

Turns the pinned LoCoMo snapshot into the unified `SuiteData` shape: one
`Instance` per conversation (sessions ordered by numeric suffix) and one
`Probe` per question, with adversarial questions marked for abstention.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

from ..fetch import ensure_dataset
from ..types import DialogueTurn, Instance, Probe, SuiteData

SUITE = "locomo"

_SESSION_KEY = re.compile(r"^session_(\d+)$")
_REQUIRED_RECORD_KEYS = ("sample_id", "conversation", "qa")
_ADVERSARIAL_CATEGORY = "5"


def _require(mapping: object, keys: tuple[str, ...], context: str) -> dict:
    """Return `mapping` if it is a dict containing every key, else ValueError."""
    if not isinstance(mapping, dict):
        raise ValueError(f"{context} must be an object, got {type(mapping).__name__}")
    missing = [key for key in keys if key not in mapping]
    if missing:
        raise ValueError(f"{context} is missing required keys: {', '.join(missing)}")
    return mapping


def _session_numbers(conversation: dict) -> list[int]:
    numbers: list[int] = []
    for key in conversation:
        match = _SESSION_KEY.match(key)
        if match is not None:
            numbers.append(int(match.group(1)))
    return sorted(numbers)


def _session_turns(
    conversation: dict, sample_id: str, number: int, speaker_a: str
) -> list[DialogueTurn]:
    context = f"conversation[{sample_id}].session_{number}"
    raw_session = conversation[f"session_{number}"]
    if not isinstance(raw_session, list):
        raise ValueError(f"{context} must be a list of turns")
    timestamp = conversation.get(f"session_{number}_date_time")
    turns: list[DialogueTurn] = []
    for index, raw_turn in enumerate(raw_session):
        turn = _require(raw_turn, ("speaker", "dia_id", "text"), f"{context}[{index}]")
        speaker = turn["speaker"]
        turns.append(
            DialogueTurn(
                session_id=f"{sample_id}:session_{number}",
                turn_id=str(turn["dia_id"]),
                speaker=str(speaker),
                role="user" if speaker == speaker_a else "assistant",
                text=str(turn["text"]),
                timestamp=timestamp,
            )
        )
    return turns


def _probe(
    item: dict, sample_id: str, index: int, categories: dict
) -> Probe:
    context = f"conversation[{sample_id}].qa[{index}]"
    item = _require(item, ("question", "category"), context)
    raw_category = item["category"]
    category = categories.get(str(raw_category), str(raw_category))
    common = {
        "probe_id": f"{sample_id}:q{index:03d}",
        "suite": SUITE,
        "instance_id": sample_id,
        "question": str(item["question"]),
        "category": category,
        "evidence_ids": tuple(item.get("evidence", [])),
    }
    if str(raw_category) == _ADVERSARIAL_CATEGORY:
        return Probe(
            **common,
            gold_answer="",
            expect_abstention=True,
            adversarial_answer=str(item.get("adversarial_answer", "")),
        )
    item = _require(item, ("answer",), context)
    return Probe(**common, gold_answer=str(item["answer"]), expect_abstention=False)


def parse(raw: list, pin: dict) -> SuiteData:
    """Parse a decoded LoCoMo snapshot into a `SuiteData`."""
    if not isinstance(raw, list) or not raw:
        raise ValueError("locomo raw must be a non-empty list of conversations")
    categories = pin.get("categories", {})
    instances: list[Instance] = []
    for index, record in enumerate(raw):
        record = _require(record, _REQUIRED_RECORD_KEYS, f"conversation[{index}]")
        sample_id = str(record["sample_id"])
        conversation = record["conversation"]
        _require(conversation, ("speaker_a",), f"conversation[{index}].conversation")
        speaker_a = conversation["speaker_a"]
        qa = record["qa"]
        if not isinstance(qa, list):
            raise ValueError(f"conversation[{index}].qa must be a list")
        sessions = [
            _session_turns(conversation, sample_id, number, speaker_a)
            for number in _session_numbers(conversation)
        ]
        probes = [
            _probe(item, sample_id, qa_index, categories)
            for qa_index, item in enumerate(qa)
        ]
        instances.append(
            Instance(instance_id=sample_id, sessions=sessions, probes=probes)
        )
    return SuiteData(suite=SUITE, instances=instances, pin=pin)


def load(pin: dict, cache_dir: Path) -> SuiteData:
    """Resolve, read, and parse the pinned LoCoMo snapshot."""
    path = ensure_dataset(pin, cache_dir)
    with open(path, "r", encoding="utf-8") as handle:
        raw = json.load(handle)
    return parse(raw, pin)
