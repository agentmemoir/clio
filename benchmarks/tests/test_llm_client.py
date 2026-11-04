"""The judge client's served-model preflight: offline, plus one loopback server."""

from __future__ import annotations

import http.server
import io
import json
import threading
import unittest
import urllib.error
from unittest import mock

from benchmarks.clio_bench.llm import (
    ChatClientError,
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
    def _client(
        self,
        *,
        base_url: str = "http://judge.test",
        model: str = "target-model",
        served_model: str = "target-served",
        revision: str = "target-artifact",
    ) -> HttpChatClient:
        return HttpChatClient(
            base_url,
            model,
            api_key="secret",
            served_model=served_model,
            revision=revision,
        )

    def test_the_preflight_requests_the_versioned_models_url(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "target-served"}]}),
        ) as opener:
            self._client().preflight()
        request = opener.call_args.args[0]
        self.assertEqual(request.full_url, "http://judge.test/v1/models")
        self.assertEqual(request.get_header("Authorization"), "Bearer secret")

    def test_the_preflight_has_its_own_short_timeout(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "target-served"}]}),
        ) as opener:
            self._client().preflight()
        self.assertEqual(opener.call_args.kwargs["timeout"], 10)

    def test_the_chat_call_shares_the_preflight_version_base(self) -> None:
        client = self._client()
        self.assertEqual(client.models_url, "http://judge.test/v1/models")
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(
                200, {"choices": [{"message": {"content": "ok"}}]}
            ),
        ) as opener:
            self.assertEqual(
                client.complete([{"role": "user", "content": "hi"}]), "ok"
            )
        request = opener.call_args.args[0]
        self.assertEqual(request.full_url, "http://judge.test/v1/chat/completions")
        self.assertEqual(opener.call_args.kwargs["timeout"], 600)

    def test_the_preflight_compares_the_served_id_not_the_request_model(self) -> None:
        """A served name that differs from the request model is what passes."""
        client = self._client(model="request-only", served_model="served-name")
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "served-name"}]}),
        ):
            client.preflight()

    def test_an_endpoint_serving_only_the_request_model_fails(self) -> None:
        """The request model is not the served name the pin records."""
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "target-model"}]}),
        ):
            with self.assertRaises(ModelUnavailableError) as ctx:
                self._client(
                    model="target-model",
                    served_model="target-served",
                    revision="artifact-revision",
                ).preflight()
        message = str(ctx.exception)
        self.assertIn("target-served", message)
        self.assertIn("http://judge.test", message)
        self.assertNotIn("target-model", message)
        self.assertNotIn("artifact-revision", message)
        self.assertNotIn("secret", message)

    def test_an_unlisted_served_model_is_unavailable(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "other-model"}]}),
        ):
            with self.assertRaises(ModelUnavailableError) as ctx:
                self._client().preflight()
        message = str(ctx.exception)
        self.assertIn("target-served", message)
        self.assertIn("http://judge.test", message)
        self.assertNotIn("target-artifact", message)
        self.assertNotIn("secret", message)

    def test_a_rejected_key_names_the_status_not_the_model(self) -> None:
        """A 401 that `urlopen` raises must not read as a missing model."""
        error = urllib.error.HTTPError(
            "http://judge.test/v1/models", 401, "Unauthorized", {}, io.BytesIO(b"")
        )
        self.addCleanup(error.close)
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen", side_effect=error
        ):
            with self.assertRaises(ChatClientError) as ctx:
                self._client().preflight()
        message = str(ctx.exception)
        self.assertIs(type(ctx.exception), ChatClientError)
        self.assertIn("rejected", message)
        self.assertIn("401", message)
        self.assertNotIn("not served", message)
        self.assertNotIn("secret", message)

    def test_a_non_2xx_response_is_rejected(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(503, {}),
        ):
            with self.assertRaises(ChatClientError) as ctx:
                self._client().preflight()
        self.assertIn("503", str(ctx.exception))

    def test_a_transport_failure_is_unreachable(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            side_effect=urllib.error.URLError("connection refused"),
        ):
            with self.assertRaises(ChatClientError) as ctx:
                self._client().preflight()
        message = str(ctx.exception)
        self.assertIn("unreachable", message)
        self.assertNotIn("not served", message)
        self.assertNotIn("secret", message)

    def test_the_api_key_is_not_sent_to_a_loopback_endpoint(self) -> None:
        for base_url in (
            "http://127.0.0.1:8899",
            "http://127.9.8.7:8899",
            "http://localhost:8899",
            "http://[::1]:8899",
        ):
            with self.subTest(base_url=base_url), mock.patch(
                "benchmarks.clio_bench.llm.urllib.request.urlopen",
                return_value=FakeResponse(200, {"data": [{"id": "target-served"}]}),
            ) as opener:
                self._client(base_url=base_url).preflight()
            request = opener.call_args.args[0]
            self.assertIsNone(request.get_header("Authorization"))

    def test_the_api_key_is_sent_to_a_non_loopback_endpoint(self) -> None:
        with mock.patch(
            "benchmarks.clio_bench.llm.urllib.request.urlopen",
            return_value=FakeResponse(200, {"data": [{"id": "target-served"}]}),
        ) as opener:
            self._client(base_url="http://judge.test").preflight()
        request = opener.call_args.args[0]
        self.assertEqual(request.get_header("Authorization"), "Bearer secret")

    def test_the_stub_is_always_available(self) -> None:
        stub = StubChatClient([], revision="stub-revision")
        self.assertEqual(stub.revision, "stub-revision")
        stub.preflight()


class _ModelsHandler(http.server.BaseHTTPRequestHandler):
    """Serve one OpenAI-compatible model list and record every path."""

    served_model = "served-revision"

    def do_GET(self) -> None:  # noqa: N802 - the base class names it
        self.server.requested_paths.append(self.path)
        if self.path != "/v1/models":
            self.send_error(404)
            return
        body = json.dumps({"data": [{"id": self.served_model}]}).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args) -> None:
        """Keep the test output quiet."""


class LoopbackPreflightTests(unittest.TestCase):
    """Prove the real request reaches `/v1/models` on a real socket."""

    def setUp(self) -> None:
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _ModelsHandler)
        self.server.requested_paths = []
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self._stop)

    def _stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def _client(self, served_model: str) -> HttpChatClient:
        base_url = f"http://127.0.0.1:{self.server.server_address[1]}"
        return HttpChatClient(
            base_url,
            "request-model",
            served_model=served_model,
            revision="artifact-revision",
        )

    def test_a_served_model_passes_over_a_real_socket(self) -> None:
        self._client("served-revision").preflight()
        self.assertEqual(self.server.requested_paths, ["/v1/models"])

    def test_an_unserved_model_fails_over_a_real_socket(self) -> None:
        with self.assertRaises(ModelUnavailableError) as ctx:
            self._client("absent-revision").preflight()
        self.assertIn("absent-revision", str(ctx.exception))
        self.assertEqual(self.server.requested_paths, ["/v1/models"])


if __name__ == "__main__":
    unittest.main()
