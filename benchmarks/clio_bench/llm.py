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


class ModelUnavailableError(ChatClientError):
    """Raised when the configured model is absent from its endpoint."""


# The environment variables that can name the frontier key, in resolution
# order. Only presence is ever reported; the value is never logged or stamped
# into a report.
KEY_ENV_VARS = ("CLIO_BENCH_API_KEY", "OPENAI_API_KEY")


def frontier_key_present(environ: dict[str, str] | None = None) -> bool:
    """Return True when a frontier judge/generator key is configured."""
    lookup = os.environ if environ is None else environ
    return any(lookup.get(name) for name in KEY_ENV_VARS)


class ChatClient(Protocol):
    """The methods the judge and answer generator need."""

    def complete(
        self,
        messages: list[dict],
        *,
        temperature: float = 0.0,
        max_tokens: int = 512,
        json_schema: dict | None = None,
    ) -> str: ...

    def preflight(self) -> None: ...


class HttpChatClient:
    """OpenAI-compatible `/v1/chat/completions` client."""

    def __init__(
        self,
        base_url: str,
        model: str,
        *,
        api_key: str | None = None,
        timeout: int = 600,
        revision: str,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.model = model
        self.revision = revision
        self._api_key = api_key or os.environ.get("CLIO_BENCH_API_KEY") or os.environ.get(
            "OPENAI_API_KEY"
        )
        self.timeout = timeout

    def _request(
        self, path: str, *, data: bytes | None = None
    ) -> urllib.request.Request:
        """Build a request on the shared base URL with the shared auth header."""
        request = urllib.request.Request(
            f"{self.base_url}{path}",
            data=data,
            headers={"Content-Type": "application/json"} if data is not None else {},
            method="POST" if data is not None else "GET",
        )
        if self._api_key:
            request.add_header("Authorization", f"Bearer {self._api_key}")
        return request

    def _send(self, request: urllib.request.Request) -> tuple[int, bytes]:
        """Send one request and return its status and body."""
        with urllib.request.urlopen(request, timeout=self.timeout) as response:
            return response.status, response.read()

    def _unavailable(self, detail: str) -> ModelUnavailableError:
        """Build the error that names the revision and endpoint, never the key."""
        return ModelUnavailableError(
            f"model revision {self.revision!r} is not available at "
            f"{self.base_url}: {detail}"
        )

    def preflight(self) -> None:
        """Fail unless the endpoint serves the configured revision id."""
        try:
            status, raw = self._send(self._request("/models"))
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise self._unavailable(f"request failed: {exc}") from exc
        if not 200 <= status < 300:
            raise self._unavailable(f"HTTP {status}")
        try:
            payload = json.loads(raw.decode("utf-8"))
            model_ids = [entry["id"] for entry in payload["data"]]
        except (json.JSONDecodeError, KeyError, TypeError) as exc:
            raise self._unavailable("the model list was not OpenAI-compatible") from exc
        if self.revision not in model_ids:
            raise self._unavailable(
                "the revision is not in the endpoint's model list"
            )

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
        try:
            _, raw = self._send(self._request("/v1/chat/completions", data=data))
            payload = json.loads(raw.decode("utf-8"))
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

    def __init__(
        self,
        replies: list[str] | Callable[[list[dict]], str],
        *,
        revision: str | None = None,
    ) -> None:
        self._replies = replies
        self.revision = revision
        self.calls: list[list[dict]] = []

    def preflight(self) -> None:
        """A stub is always available."""

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