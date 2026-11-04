"""Minimal OpenAI-compatible chat client used by the judge and answer generator.

Uses only the standard library so the harness has no third-party runtime
dependency. Works against a local llama.cpp server or a hosted endpoint.
Credentials come from the environment (`CLIO_BENCH_API_KEY`, falling back to
`OPENAI_API_KEY`) and are never logged or written to reports. A loopback
endpoint never receives the frontier key.
"""

from __future__ import annotations

import ipaddress
import json
import os
import urllib.error
import urllib.parse
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


# The one version segment both the models list and the chat call use, so the
# two requests cannot drift onto different paths again.
API_VERSION = "/v1"

# Names and address ranges that mean "this machine". A judge served here must
# never receive the frontier key.
_LOOPBACK_HOSTS = frozenset({"localhost"})
_LOOPBACK_NETWORKS = (
    ipaddress.ip_network("127.0.0.0/8"),
    ipaddress.ip_network("::1/128"),
)


def _is_loopback_url(base_url: str) -> bool:
    """Return True when the URL host is localhost, 127.0.0.0/8, or ::1."""
    host = urllib.parse.urlsplit(base_url).hostname
    if not host:
        return False
    if host.lower() in _LOOPBACK_HOSTS:
        return True
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        return False
    return any(address in network for network in _LOOPBACK_NETWORKS)


class HttpChatClient:
    """OpenAI-compatible `/v1/chat/completions` client."""

    def __init__(
        self,
        base_url: str,
        model: str,
        *,
        served_model: str,
        revision: str,
        api_key: str | None = None,
        timeout: int = 600,
        preflight_timeout: int = 10,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.model = model
        self.served_model = served_model
        self.revision = revision
        self._loopback = _is_loopback_url(self.base_url)
        self._api_key = api_key or os.environ.get("CLIO_BENCH_API_KEY") or os.environ.get(
            "OPENAI_API_KEY"
        )
        self.timeout = timeout
        self.preflight_timeout = preflight_timeout

    @property
    def api_base(self) -> str:
        """The versioned base that both request paths are built from."""
        return f"{self.base_url}{API_VERSION}"

    @property
    def models_url(self) -> str:
        """The exact URL the preflight requests the model list from."""
        return f"{self.api_base}/models"

    def _request(
        self, path: str, *, data: bytes | None = None
    ) -> urllib.request.Request:
        """Build a request on the shared versioned base and auth header."""
        request = urllib.request.Request(
            f"{self.api_base}{path}",
            data=data,
            headers={"Content-Type": "application/json"} if data is not None else {},
            method="POST" if data is not None else "GET",
        )
        if self._api_key and not self._loopback:
            request.add_header("Authorization", f"Bearer {self._api_key}")
        return request

    def _send(
        self, request: urllib.request.Request, *, timeout: int
    ) -> tuple[int, bytes]:
        """Send one request and return its status and body."""
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read()

    def _unreachable(self, exc: Exception) -> ChatClientError:
        """Build the error for a transport failure, never naming the key."""
        return ChatClientError(f"model endpoint {self.api_base} unreachable: {exc}")

    def _rejected(self, status: int) -> ChatClientError:
        """Build the error for a status the endpoint returned on purpose."""
        return ChatClientError(
            f"model endpoint {self.api_base} rejected the request with HTTP {status}"
        )

    def _unavailable(self, detail: str) -> ModelUnavailableError:
        """Build the error that names the served model and endpoint, never the key."""
        return ModelUnavailableError(
            f"served model {self.served_model!r} is not served at "
            f"{self.models_url}: {detail}"
        )

    def preflight(self) -> None:
        """Fail unless the endpoint serves the configured served model id."""
        try:
            status, raw = self._send(
                self._request("/models"), timeout=self.preflight_timeout
            )
        except urllib.error.HTTPError as exc:
            raise self._rejected(exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise self._unreachable(exc) from exc
        if not 200 <= status < 300:
            raise self._rejected(status)
        try:
            payload = json.loads(raw.decode("utf-8"))
            model_ids = [entry["id"] for entry in payload["data"]]
        except (json.JSONDecodeError, KeyError, TypeError, UnicodeDecodeError) as exc:
            raise self._unavailable("the model list was not OpenAI-compatible") from exc
        if self.served_model not in model_ids:
            raise self._unavailable(
                "the model list does not include the served model"
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
            _, raw = self._send(
                self._request("/chat/completions", data=data), timeout=self.timeout
            )
            payload = json.loads(raw.decode("utf-8"))
        except urllib.error.HTTPError as exc:
            raise self._rejected(exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise self._unreachable(exc) from exc
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