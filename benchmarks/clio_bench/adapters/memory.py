"""In-memory reference adapter used for offline runs and tests.

It implements the same interface as the Clio adapter with no external
dependencies, so integration and end-to-end tests are deterministic and
hermetic. Retrieval is lexical overlap over stored turns, truncated to the
canonical token budget.
"""

from __future__ import annotations

from ..adapter import MemoryBenchmarkAdapter
from ..fingerprint import Fingerprint, unreadable_setting
from ..scoring import _normalize
from ..tokenizer import ReferenceTokenizer
from ..types import DialogueTurn, IngestResult, RetrievalResult

# The double exposes no settings surface, so its fingerprint minimum is empty.
REQUIRED_SETTINGS = ()


class MemoryAdapter(MemoryBenchmarkAdapter):
    """A small deterministic adapter: store turns, retrieve by overlap."""

    def __init__(self, tokenizer: ReferenceTokenizer, *, top_k: int = 8) -> None:
        self._tokenizer = tokenizer
        self._top_k = top_k
        self._banks: dict[str, list[DialogueTurn]] = {}

    def reset(self, session_id: str) -> None:
        self._banks[session_id] = []

    def ingest(self, session_id: str, turn: DialogueTurn) -> IngestResult:
        self._banks.setdefault(session_id, []).append(turn)
        return IngestResult(
            ok=True,
            latency_ms=0.0,
            tokens_processed=self._tokenizer.count(turn.text),
        )

    def retrieve(
        self, session_id: str, query: str, token_budget: int
    ) -> RetrievalResult:
        query_terms = set(_normalize(query))
        turns = self._banks.get(session_id, [])
        scored = []
        for turn in turns:
            overlap = len(query_terms & set(_normalize(turn.text)))
            if overlap:
                scored.append((overlap, turn))
        scored.sort(key=lambda pair: (-pair[0], pair[1].turn_id))
        chosen = [turn for _, turn in scored[: self._top_k]]
        pieces: list[str] = []
        used = 0
        for turn in chosen:
            cost = self._tokenizer.count(turn.text)
            if used + cost > token_budget:
                break
            pieces.append(f"[{turn.turn_id}] {turn.speaker}: {turn.text}")
            used += cost
        return RetrievalResult(
            context="\n".join(pieces),
            latency_ms=0.0,
            tokens_retrieved=used,
            item_ids=tuple(turn.turn_id for turn in chosen[: len(pieces)]),
        )

    def healthcheck(self) -> bool:
        return True

    def fingerprint(self) -> Fingerprint:
        """The double has no configuration, and says so instead of guessing.

        Its minimum is empty because it exposes no settings surface; the one
        record is the explicit absence the shared shape asks for.
        """
        return Fingerprint(
            system="memory",
            version="unversioned in-process double",
            settings={
                "configurable_settings": unreadable_setting(
                    tried=(
                        "in-process deterministic double: no settings surface exists",
                    )
                )
            },
            required=(),
        )