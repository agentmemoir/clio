"""Unified system-adapter interface.

Mirrors the contract in the benchmark strategy: every benchmarked system
(Clio and future rivals) implements `reset`, `ingest`, `retrieve`, and an
optional `answer`, plus `healthcheck` and `fingerprint`.
"""

from __future__ import annotations

from abc import ABC, abstractmethod

from .fingerprint import Fingerprint
from .types import DialogueTurn, IngestResult, RetrievalResult


class AdapterError(RuntimeError):
    """Raised when an adapter cannot carry out an operation at all.

    Distinct from the system under test declining a write on its own rules,
    which the adapter records as `IngestResult(ok=False)`. An `AdapterError`
    means the harness could not do what it set out to do, so a run that hits
    one stops instead of scoring what it managed to collect.
    """


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
        """Return a context string for `query` within a hard token budget.

        The budget bounds the context that is returned, so `tokens_retrieved`
        is the cost of that context string itself and never more than the
        budget it was given.
        """

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
