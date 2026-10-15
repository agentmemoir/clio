"""Live HTTP transport, execution, and diagnostic capture."""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import socket
from pathlib import Path
from typing import Any, Mapping, Sequence
from urllib.error import HTTPError, URLError
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from extract_quality_core import (
    MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES,
    CaseResult,
    FixtureCase,
    FixtureSet,
    HarnessError,
    SuiteReport,
)
from extract_quality_evaluate import _case_result
from extract_quality_redact import _json_text, redact_json
from extract_quality_request import (
    COMPLETION_INSTRUCTIONS,
    SYSTEM_PROMPT,
    _endpoint_for_request,
    build_request,
    build_user_message,
)

__all__ = [
    "COMPLETION_INSTRUCTIONS",
    "SYSTEM_PROMPT",
    "build_request",
    "build_user_message",
    "run_live",
]


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


_NO_REDIRECT_OPENER = build_opener(ProxyHandler({}), _NoRedirect)


def _post_json(endpoint: str, body: Mapping[str, Any], timeout: float) -> tuple[bytes, int | None]:
    try:
        encoded = _json_text(body).encode("utf-8")
    except (TypeError, ValueError) as exc:
        raise HarnessError("request_invalid", "request is not valid JSON") from exc
    if len(encoded) > MAX_REQUEST_BYTES:
        raise HarnessError("request_too_large", "request exceeds the size limit")
    request = Request(
        endpoint,
        data=encoded,
        headers={
            "Accept": "application/json",
            "Content-Type": "application/json",
            "User-Agent": "clio-extract-quality/1",
        },
        method="POST",
    )
    try:
        with _NO_REDIRECT_OPENER.open(request, timeout=timeout) as response:  # nosec B310 - explicit http(s) validation above
            status = getattr(response, "status", None) or response.getcode()
            declared = getattr(response, "headers", {}).get("Content-Length")
            if declared is not None:
                try:
                    if int(declared) > MAX_RESPONSE_BYTES:
                        raise HarnessError("response_too_large", "response exceeds the size limit")
                except ValueError as exc:
                    raise HarnessError("malformed_response", "invalid Content-Length") from exc
            data = response.read(MAX_RESPONSE_BYTES + 1)
    except HarnessError:
        raise
    except HTTPError as exc:
        raise HarnessError("provider_error", f"HTTP status {exc.code}") from exc
    except (TimeoutError, socket.timeout) as exc:
        raise HarnessError("endpoint_timeout", "endpoint request timed out") from exc
    except URLError as exc:
        reason = getattr(exc, "reason", None)
        if isinstance(reason, (TimeoutError, socket.timeout)):
            raise HarnessError("endpoint_timeout", "endpoint request timed out") from exc
        raise HarnessError("endpoint_unavailable", "endpoint could not be reached") from exc
    except OSError as exc:
        raise HarnessError("endpoint_unavailable", "endpoint could not be reached") from exc
    except ValueError as exc:
        raise HarnessError("endpoint_invalid", "endpoint URL is invalid") from exc
    if len(data) > MAX_RESPONSE_BYTES:
        raise HarnessError("response_too_large", "response exceeds the size limit")
    return data, int(status) if status is not None else None


def _decode_response(data: bytes) -> Any:
    if not data.strip():
        raise HarnessError("empty_output", "endpoint returned an empty body")
    if len(data) > MAX_RESPONSE_BYTES:
        raise HarnessError("response_too_large", "response exceeds the size limit")
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise HarnessError("malformed_response", "response is not UTF-8") from exc
    try:
        return json.loads(text)
    except (json.JSONDecodeError, RecursionError) as exc:
        raise HarnessError("malformed_response", "response is not JSON") from exc


def _safe_raw_record(
    case: FixtureCase,
    data: bytes | None,
    status: int | None,
    cause: str | None,
    parsed: Any | None,
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "format": "clio-extract-raw-v1",
        "case_id": case.case_id,
        "http_status": status,
        "cause": cause,
        "body_bytes": len(data or b""),
        "body_sha256": hashlib.sha256(data or b"").hexdigest() if data is not None else None,
    }
    if parsed is not None:
        record["response"] = redact_json(parsed)
    elif data:
        # Keep malformed/untrusted bytes out of the diagnostic file.  The
        # digest and length still identify the response without echoing it.
        record["body_preview"] = "[omitted: response was not valid JSON]"
    return record


def _write_raw_records(path: Path, records: Sequence[Mapping[str, Any]]) -> None:
    payload = {"format": "clio-extract-raw-v1", "records": list(records)}
    try:
        encoded = (_json_text(redact_json(payload)) + "\n").encode("utf-8")
        if len(encoded) > MAX_RESPONSE_BYTES:
            raise HarnessError("raw_record_too_large", "raw diagnostic record exceeds the size limit")
        path.parent.mkdir(parents=True, exist_ok=True)
        flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(path, flags, 0o600)
        with os.fdopen(fd, "wb") as handle:
            handle.write(encoded)
        with contextlib.suppress(OSError):
            os.chmod(path, 0o600)
    except HarnessError:
        raise
    except (OSError, TypeError, ValueError) as exc:
        raise HarnessError("raw_record_io", "could not write redacted raw record") from exc


def _positive_cases(fixture_sets: Sequence[FixtureSet]) -> list[FixtureCase]:
    cases = [case for fixture_set in fixture_sets for case in fixture_set.cases if not case.control]
    if not cases:
        raise HarnessError("fixture_schema", "live mode has no positive fixture cases")
    return cases


def _live_failure(case: FixtureCase, cause: str) -> CaseResult:
    return CaseResult(
        case_id=case.case_id,
        suite="live",
        expected_admission=True,
        expected_grounding=True,
        control=False,
        extracted=False,
        structural_admitted=False,
        admitted=False,
        grounded=False,
        exact=False,
        gold_hit=0,
        gold_total=len(case.gold),
        success=False,
        cause=cause,
        expected_failures=[{"field": "<live>", "reason": cause}],
    )


def run_live(
    fixture_sets: Sequence[FixtureSet],
    endpoint: str,
    model: str,
    temperature: float,
    timeout: float,
    request_shape: str,
    raw_output: Path | None,
    no_retry: bool = False,
) -> SuiteReport:
    cases = _positive_cases(fixture_sets)
    available_cases = sum(len(fixture_set.cases) for fixture_set in fixture_sets)
    try:
        url = _endpoint_for_request(endpoint, request_shape)
    except HarnessError as exc:
        results = [_live_failure(case, exc.cause) for case in cases]
        raw_records = [
            _safe_raw_record(case, None, None, exc.cause, None) for case in cases
        ]
        return SuiteReport(
            name="live",
            results=results,
            fixture_count=available_cases,
            contract_count=0,
            raw_records=raw_records,
            evaluated_fixture_count=len(cases),
            skipped_fixture_cases=available_cases - len(cases),
            evidence="live_provider_attempt",
            metric_label="live_fidelity",
        )
    results: list[CaseResult] = []
    raw_records: list[dict[str, Any]] = []
    endpoint_failure: str | None = None
    for case in cases:
        data: bytes | None = None
        status: int | None = None
        parsed: Any | None = None
        if endpoint_failure is not None:
            result = _live_failure(case, endpoint_failure)
        else:
            attempts = 1 if no_retry else 2
            result = _live_failure(case, "request_invalid")
            for _attempt in range(attempts):
                data = None
                status = None
                parsed = None
                try:
                    body = build_request(case, model, temperature, request_shape)
                    data, status = _post_json(url, body, timeout)
                    parsed = _decode_response(data)
                    result = _case_result(case, "live", parsed)
                    if result.success:
                        break
                except HarnessError as exc:
                    result = _live_failure(case, exc.cause)
                    if exc.cause in {"endpoint_unavailable", "endpoint_timeout"}:
                        endpoint_failure = exc.cause
                        break
                except (TypeError, ValueError):
                    result = _live_failure(case, "request_invalid")
        result.raw_record = _safe_raw_record(case, data, status, result.cause, parsed)
        raw_records.append(result.raw_record)
        results.append(result)
    if raw_output is not None:
        _write_raw_records(raw_output, raw_records)
    return SuiteReport(
        name="live",
        results=results,
        fixture_count=available_cases,
        contract_count=0,
        raw_records=raw_records,
        evaluated_fixture_count=len(cases),
        skipped_fixture_cases=available_cases - len(cases),
        evidence="live_provider_attempt",
        metric_label="live_fidelity",
    )
