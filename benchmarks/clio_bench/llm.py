"""Minimal OpenAI-compatible chat client used by the judge and answer generator.

Uses only the standard library so the harness has no third-party runtime
dependency. Works against a local llama.cpp server or a hosted endpoint.
Credentials come from the environment (`CLIO_BENCH_API_KEY`, falling back to
`OPENAI_API_KEY`) and are never logged or written to reports.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from typing import Callable, Protocol


class ChatClientError(RuntimeError):
    """Raised when the model endpoint is unreachable or returns an error."""


# The environment variables that can name the frontier key, in resolution
# order. Only presence is ever reported; the value is never logged or stamped
# into a report.
KEY_ENV_VARS = ("CLIO_BENCH_API_KEY", "OPENAI_API_KEY")


def frontier_key_present(environ: dict[str, str] | None = None) -> bool:
    """Return True when a frontier judge/generator key is configured."""
    lookup = os.environ if environ is None else environ
    return any(lookup.get(name) for name in KEY_ENV_VARS)


class ChatClient(Protocol):
    """The one method the judge and answer generator need."""

    def complete(
        self,
        messages: list[dict],
        *,
        temperature: float = 0.0,
        max_tokens: int = 512,
        json_schema: dict | None = None,
    ) -> str: ...


class HttpChatClient:
    """OpenAI-compatible `/v1/chat/completions` client."""

    def __init__(
        self,
        base_url: str,
        model: str,
        *,
        api_key: str | None = None,
        timeout: int = 600,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.model = model
        self._api_key = api_key or os.environ.get("CLIO_BENCH_API_KEY") or os.environ.get(
            "OPENAI_API_KEY"
        )
        self.timeout = timeout

    def complete(
        self,
        messages: list[dict],
        *,
        temperature: float = 0.0,
        max_tokens: int = 512,
        json_schema: dict | None = None,
    ) -> str:
        body: dict = {
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
        }
        if json_schema is not None:
            body["response_format"] = {
                "type": "json_schema",
                "json_schema": {"name": "clio_bench_verdict", "schema": json_schema},
            }
        data = json.dumps(body).encode("utf-8")
        request = urllib.request.Request(
            f"{self.base_url}/v1/chat/completions",
            data=data,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        if self._api_key:
            request.add_header("Authorization", f"Bearer {self._api_key}")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise ChatClientError(
                f"model endpoint {self.base_url} unreachable: {exc}"
            ) from exc
        except json.JSONDecodeError as exc:
            raise ChatClientError(f"model endpoint returned non-JSON: {exc}") from exc
        try:
            return payload["choices"][0]["message"]["content"]
        except (KeyError, IndexError, TypeError) as exc:
            raise ChatClientError(f"unexpected model response shape: {exc}") from exc


class StubChatClient:
    """Deterministic client for tests and offline runs.

    Accepts either a list of canned replies (cycled) or a callable that maps
    the message list to a reply.
    """

    def __init__(self, replies: list[str] | Callable[[list[dict]], str]) -> None:
        self._replies = replies
        self.calls: list[list[dict]] = []

    def complete(
        self,
        messages: list[dict],
        *,
        temperature: float = 0.0,
        max_tokens: int = 512,
        json_schema: dict | None = None,
    ) -> str:
        self.calls.append(messages)
        if callable(self._replies):
            return self._replies(messages)
        if not self._replies:
            raise ChatClientError("StubChatClient has no replies queued")
        index = min(len(self.calls) - 1, len(self._replies) - 1)
        return self._replies[index]