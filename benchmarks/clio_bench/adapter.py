"""Unified system-adapter interface.

Mirrors the contract in the benchmark strategy: every benchmarked system
(Clio and future rivals) implements `reset`, `ingest`, `retrieve`, and an
optional `answer`, plus `healthcheck` and `fingerprint`.
"""

from __future__ import annotations

from abc import ABC, abstractmethod

from .fingerprint import Fingerprint
from .types import DialogueTurn, IngestResult, RetrievalResult


class MemoryBenchmarkAdapter(ABC):
    """The interface every benchmarked system adapter implements."""

    @abstractmethod
    def reset(self, session_id: str) -> None:
        """Clear state and prepare a fresh memory bank for a test case."""

    @abstractmethod
    def ingest(self, session_id: str, turn: DialogueTurn) -> IngestResult:
        """Ingest one conversation turn; return latency, tokens, and success."""

    @abstractmethod
    def retrieve(
        self, session_id: str, query: str, token_budget: int
    ) -> RetrievalResult:
        """Return a context string for `query` within a hard token budget."""

    def answer(self, session_id: str, query: str, generator_llm: str) -> str:
        """Optional end-to-end answering using the system's own retrieval."""
        raise NotImplementedError(
            f"{type(self).__name__} does not implement end-to-end answering"
        )

    @abstractmethod
    def healthcheck(self) -> bool:
        """Return True when the system is reachable and ready to be driven."""

    @abstractmethod
    def fingerprint(self) -> Fingerprint:
        """Return the settings that change this system's behaviour.

        The runner calls this before any probe runs and stamps the result into
        the report. An adapter reads its system's own surfaces and records a
        setting with no surface as explicitly unreadable, naming what was
        tried; it never omits such a field.
        """
