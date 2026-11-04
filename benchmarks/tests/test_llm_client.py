"""The judge client's model preflight: offline, no socket is opened."""

from __future__ import annotations

import json
import unittest
import urllib.error
from unittest import mock

from benchmarks.clio_bench.llm import (
    HttpChatClient,
    ModelUnavailableError,
    StubChatClient,
)


class FakeResponse:
    """Minimal stand-in for the `urlopen` context manager."""

    def __init__(self, status: int, payload: dict) -> None:
        self.status = status
        self._body = json.dumps(payload).encode("utf-8")

    def read(self) -> bytes:
        return self._body

    def __enter__(self) -> "FakeResponse":
        return self

    def __exit__(self, *exc) -> bool:
        return False


class PreflightTests(unittest.TestCase):
    def _client(self) -> HttpChatClient:
        return HttpChatClient("http://judge.test", "target-model", api_key="secret")

    def test_a_listed_model_passes(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "target-model"}]}),
        ) as opener:
            self._client().preflight()
        request = opener.call_args.args[0]
        self.assertTrue(request.full_url.endswith("/models"))
        self.assertEqual(request.get_header("Authorization"), "Bearer secret")

    def test_an_unlisted_model_is_unavailable(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "other-model"}]}),
        ):
            with self.assertRaises(ModelUnavailableError) as ctx:
                self._client().preflight()
        message = str(ctx.exception)
        self.assertIn("target-model", message)
        self.assertIn("http://judge.test", message)
        self.assertNotIn("secret", message)

    def test_a_non_2xx_response_is_unavailable(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(503, {}),
        ):
            with self.assertRaises(ModelUnavailableError):
                self._client().preflight()

    def test_a_transport_failure_is_unavailable(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            side_effect=urllib.error.URLError("connection refused"),
        ):
            with self.assertRaises(ModelUnavailableError):
                self._client().preflight()

    def test_the_stub_is_always_available(self) -> None:
        StubChatClient([]).preflight()


if __name__ == "__main__":
    unittest.main()
