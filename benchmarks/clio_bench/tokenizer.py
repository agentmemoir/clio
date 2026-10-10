"""Canonical reference tokenizer for all token-budget metrics.

The benchmark strategy pins `tiktoken` with the `cl100k_base` encoding as the
universal standard. `tiktoken` is a native extension and is not installed in
every environment, so this module uses it when importable and otherwise falls
back to a deterministic, documented estimator. Every report records which
tokenizer actually ran so token numbers are never silently incomparable.
"""

from __future__ import annotations

import re

_TOKEN_RE = re.compile(r"\w+|[^\w\s]", re.UNICODE)


class ReferenceTokenizer:
    """Counts tokens with the pinned encoding, or a named deterministic fallback."""

    def __init__(self, name: str, counter) -> None:
        self.name = name
        self._counter = counter

    def count(self, text: str) -> int:
        if not text:
            return 0
        return max(1, int(self._counter(text)))


def _fallback_count(text: str) -> int:
    """Deterministic approximation of cl100k word/punctuation tokens.

    Splits on word characters and individual punctuation, which tracks
    cl100k_base closely for English dialogue while staying reproducible.
    """
    return len(_TOKEN_RE.findall(text))


def get_tokenizer(encoding: str = "cl100k_base") -> ReferenceTokenizer:
    """Return the pinned tokenizer when available, else the fallback.

    `encoding` must be `cl100k_base`: the harness pins that encoding and does
    not accept a different one, so token metrics stay comparable.
    """
    if encoding != "cl100k_base":
        raise ValueError(f"clio-bench pins cl100k_base, got {encoding!r}")
    try:  # pragma: no cover - depends on the host environment
        import tiktoken  # type: ignore

        try:
            encoder = tiktoken.get_encoding("cl100k_base")
        except Exception:  # noqa: BLE001 - network/BPE fetch can fail at runtime
            encoder = None
        if encoder is not None:
            return ReferenceTokenizer(
                "cl100k_base(tiktoken)", lambda text: len(encoder.encode(text))
            )
    except ImportError:
        pass
    return ReferenceTokenizer("cl100k_base(clio-bench-fallback)", _fallback_count)