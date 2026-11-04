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
    def _client(self, revision: str = "target-revision") -> HttpChatClient:
        return HttpChatClient(
            "http://judge.test",
            "target-model",
            api_key="secret",
            revision=revision,
        )

    def test_a_listed_revision_passes(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "target-revision"}]}),
        ) as opener:
            self._client().preflight()
        request = opener.call_args.args[0]
        self.assertTrue(request.full_url.endswith("/models"))
        self.assertEqual(request.get_header("Authorization"), "Bearer secret")

    def test_the_served_id_is_the_revision_not_the_request_model(self) -> None:
        """The endpoint lists the request model, but not the pinned revision."""
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "target-model"}]}),
        ):
            with self.assertRaises(ModelUnavailableError) as ctx:
                self._client(revision="dated-snapshot").preflight()
        message = str(ctx.exception)
        self.assertIn("dated-snapshot", message)
        self.assertNotIn("target-model", message)

    def test_an_unlisted_revision_is_unavailable(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "other-model"}]}),
        ):
            with self.assertRaises(ModelUnavailableError) as ctx:
                self._client().preflight()
        message = str(ctx.exception)
        self.assertIn("target-revision", message)
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
        stub = StubChatClient([], revision="stub-revision")
        self.assertEqual(stub.revision, "stub-revision")
        stub.preflight()


if __name__ == "__main__":
    unittest.main()
