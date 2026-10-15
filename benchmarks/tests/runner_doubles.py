"""Adapters the runner tests drive to exercise failure and decline paths.

`DecliningAdapter` declines every write for one named reason, `FailingWriteAdapter` fails every write with a configuration error, and `FlakyWriteAdapter` fails only its first write. More than one runner test module drives them, so they live here rather than being defined in each.
"""

from __future__ import annotations

from dataclasses import replace

from benchmarks.clio_bench.adapters.clio import ClioAdapterError
from benchmarks.clio_bench.adapters.memory import MemoryAdapter

FRACTIONS = (0.6, 0.2, 0.2)


class DecliningAdapter(MemoryAdapter):
    """An adapter whose every write is declined for one named reason."""

    def __init__(self, tokenizer, refusal: str) -> None:
        super().__init__(tokenizer)
        self._refusal = refusal

    def ingest(self, session_id, turn):
        stored = super().ingest(session_id, turn)
        return replace(
            stored,
            ok=False,
            error=f"declined as {self._refusal}",
            refusal=self._refusal,
        )


class FailingWriteAdapter(MemoryAdapter):
    """An adapter whose writes fail for a reason that is not a decline.

    This is the shape of the issue #21 reproduction: `clio remember` exits
    non-zero with a `config_corrupt` payload when the data directory is
    relative, and the harness used to record every such write as an admission
    refusal and then score the empty store.
    """

    def ingest(self, session_id, turn):
        raise ClioAdapterError(
            "clio remember failed with code 'config_corrupt': "
            "no key-file path could be resolved"
        )


class FlakyWriteAdapter(MemoryAdapter):
    """An adapter that fails one write and stores the rest."""

    def __init__(self, tokenizer) -> None:
        super().__init__(tokenizer)
        self._calls = 0

    def ingest(self, session_id, turn):
        self._calls += 1
        if self._calls == 1:
            raise ClioAdapterError("clio remember failed with code 'internal': boom")
        return super().ingest(session_id, turn)
