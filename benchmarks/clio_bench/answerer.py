"""Grounded answer generator for clio-bench.

The generator makes a single, deterministic call to the chat client and is
constrained to answer only from the supplied retrieval context. When the
model returns an empty answer the generator falls back to an explicit
"not known" sentence instead of returning an empty string.
"""

from __future__ import annotations

from .llm import ChatClient

_NOT_KNOWN_FALLBACK = "The answer is not known from the provided context."

_SYSTEM_PROMPT = (
    "You are a careful question-answering assistant. Answer the user's question "
    "using ONLY the provided context. Do not use outside knowledge and do not "
    "guess. If the context does not contain the answer, reply that the answer is "
    "not known."
)


class AnswerGenerator:
    """Single-call, context-grounded answer generator."""

    def __init__(self, client: ChatClient, model: str, *, max_tokens: int = 256) -> None:
        self.client = client
        self.model = model
        self.max_tokens = max_tokens

    def answer(self, question: str, context: str) -> str:
        """Return a grounded answer, or a short fallback when the model is empty."""
        user = (
            f"Context:\n{context}\n\n"
            f"Question: {question}\n\n"
            "Answer using only the context above. If the answer is not present, "
            "say that it is not known."
        )
        messages = [
            {"role": "system", "content": _SYSTEM_PROMPT},
            {"role": "user", "content": user},
        ]
        content = self.client.complete(
            messages,
            temperature=0.0,
            max_tokens=self.max_tokens,
        )
        stripped = content.strip()
        if not stripped:
            return _NOT_KNOWN_FALLBACK
        return stripped
