"""LongMemEval oracle suite adapter for clio-bench.

Turns the pinned LongMemEval oracle snapshot into the unified `SuiteData`
shape: one `Instance` per haystack item and exactly one `Probe` per item.
"""

from __future__ import annotations

import json
from pathlib import Path

from ..fetch import ensure_dataset
from ..types import DialogueTurn, Instance, Probe, SuiteData

SUITE = "longmemeval"

_REQUIRED_ITEM_KEYS = (
    "question_id",
    "question_type",
    "question",
    "answer",
    "haystack_session_ids",
    "haystack_dates",
    "haystack_sessions",
)


def _require(mapping: object, keys: tuple[str, ...], context: str) -> dict:
    """Return `mapping` if it is a dict containing every key, else ValueError."""
    if not isinstance(mapping, dict):
        raise ValueError(f"{context} must be an object, got {type(mapping).__name__}")
    missing = [key for key in keys if key not in mapping]
    if missing:
        raise ValueError(f"{context} is missing required keys: {', '.join(missing)}")
    return mapping


def _session_turns(item: dict, context: str) -> list[list[DialogueTurn]]:
    session_ids = item["haystack_session_ids"]
    dates = item["haystack_dates"]
    raw_sessions = item["haystack_sessions"]
    if not (
        isinstance(session_ids, list)
        and isinstance(dates, list)
        and isinstance(raw_sessions, list)
    ):
        raise ValueError(f"{context} haystack_session_ids/dates/sessions must be lists")
    if not len(session_ids) == len(dates) == len(raw_sessions):
        raise ValueError(
            f"{context} haystack_session_ids/dates/sessions lengths differ"
        )
    sessions: list[list[DialogueTurn]] = []
    for session_id, date, raw_session in zip(session_ids, dates, raw_sessions):
        sid = str(session_id)
        if not isinstance(raw_session, list):
            raise ValueError(f"{context} session {sid} must be a list of turns")
        turns: list[DialogueTurn] = []
        for index, raw_turn in enumerate(raw_session):
            turn = _require(
                raw_turn, ("role", "content"), f"{context} session {sid}[{index}]"
            )
            role = str(turn["role"])
            turns.append(
                DialogueTurn(
                    session_id=sid,
                    turn_id=f"{sid}:t{index:03d}",
                    speaker=role,
                    role=role,
                    text=str(turn["content"]),
                    timestamp=date,
                )
            )
        sessions.append(turns)
    return sessions


def parse(raw: list, pin: dict) -> SuiteData:
    """Parse a decoded LongMemEval oracle snapshot into a `SuiteData`."""
    if not isinstance(raw, list) or not raw:
        raise ValueError("longmemeval raw must be a non-empty list of items")
    categories = pin.get("categories", {})
    instances: list[Instance] = []
    for index, raw_item in enumerate(raw):
        context = f"item[{index}]"
        item = _require(raw_item, _REQUIRED_ITEM_KEYS, context)
        question_id = str(item["question_id"])
        question_type = str(item["question_type"])
        probes = [
            Probe(
                probe_id=question_id,
                suite=SUITE,
                instance_id=question_id,
                question=str(item["question"]),
                gold_answer=str(item["answer"]),
                category=categories.get(question_type, question_type),
                evidence_ids=tuple(item.get("answer_session_ids", [])),
                expect_abstention=False,
            )
        ]
        instances.append(
            Instance(
                instance_id=question_id,
                sessions=_session_turns(item, context),
                probes=probes,
            )
        )
    return SuiteData(suite=SUITE, instances=instances, pin=pin)


def load(pin: dict, cache_dir: Path) -> SuiteData:
    """Resolve, read, and parse the pinned LongMemEval oracle snapshot."""
    path = ensure_dataset(pin, cache_dir)
    with open(path, "r", encoding="utf-8") as handle:
        raw = json.load(handle)
    return parse(raw, pin)
