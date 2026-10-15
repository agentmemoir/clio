"""Live HTTP request building, transport, and diagnostic capture."""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import socket
from pathlib import Path
from typing import Any, Mapping, Sequence
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse, urlunparse
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
from extract_quality_redact import _json_text, _redact_text, redact_json

SYSTEM_PROMPT = (
    "You are a memory extraction engine. Inspect every fact in the source "
    "text and return one JSON object. Populate every field in the supplied "
    "schema when a source span supports it. Treat schema keys as labels, not "
    "values. Use an exact source span for every populated field. Copy names, "
    "numbers, dates, and version strings exactly from the source; do not "
    "paraphrase, round, or reformat them. Return only snapshot, gist, "
    "source_ref, and diagnostic."
)
COMPLETION_INSTRUCTIONS = (
    "Inspect every fact before extracting. Populate every supported template "
    "field even when its key is an abstract label. Treat keys as labels, not "
    "values, and copy each value from an exact source span."
)


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


_NO_REDIRECT_OPENER = build_opener(ProxyHandler({}), _NoRedirect)


def _endpoint_for_request(endpoint: str, request_shape: str) -> str:
    endpoint = endpoint.strip()
    try:
        parsed = urlparse(endpoint)
    except ValueError as exc:
        raise HarnessError("endpoint_invalid", "endpoint must be an explicit http(s) URL") from exc
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        raise HarnessError("endpoint_invalid", "endpoint must be an explicit http(s) URL")
    if parsed.username or parsed.password:
        raise HarnessError("endpoint_invalid", "endpoint credentials are not accepted")
    if request_shape not in {"openai", "completion"}:
        return endpoint
    path = parsed.path.rstrip("/")
    if request_shape == "completion":
        if not path.endswith("/completion"):
            path = (path or "") + "/completion"
    else:
        if path.endswith("/v1/chat/completions"):
            return endpoint
        if path.endswith("/v1"):
            path = path[:-3]
        path = (path or "") + "/v1/chat/completions"
    return urlunparse((parsed.scheme, parsed.netloc, path, "", "", ""))


def _neutralize_delimiters(text: str) -> str:
    for tag in ("schema_template", "tool_payload", "source_text", "verify_feedback"):
        text = text.replace(f"<{tag}>", f"[{tag}]")
        text = text.replace(f"</{tag}>", f"[/{tag}]")
    return text


def build_user_message(case: FixtureCase) -> str:
    sections = [
        "<schema_template>\n"
        + _neutralize_delimiters(_json_text(case.schema))
        + "\n</schema_template>"
    ]
    if case.tool_payload is not None:
        sections.append(
            "<tool_payload>\n"
            + _neutralize_delimiters(_json_text(redact_json(case.tool_payload)))
            + "\n</tool_payload>"
        )
    sections.append(
        "<source_text>\n"
        + _neutralize_delimiters(_redact_text(case.source_text, limit=None))
        + "\n</source_text>"
    )
    if case.verify_feedback:
        sections.append(
            "<verify_feedback>\n"
            + _neutralize_delimiters(_redact_text(case.verify_feedback))
            + "\n</verify_feedback>"
        )
    return "\n".join(sections)


def build_request(case: FixtureCase, model: str, temperature: float, request_shape: str) -> dict[str, Any]:
    if request_shape == "template":
        return {
            "model": model,
            "temperature": temperature,
            "template": case.schema,
            "text": _redact_text(case.source_text, limit=None),
            "tool_payload": redact_json(case.tool_payload),
            "verify_feedback": _redact_text(case.verify_feedback) if case.verify_feedback else None,
        }
    if request_shape == "completion":
        template = _json_text(case.schema)
        source = _redact_text(case.source_text, limit=None)
        prompt = (
            "<|input|>\n### Instructions:\n"
            f"{COMPLETION_INSTRUCTIONS}\n### Template:\n"
            f"{template}\n### Text:\n{source}\n\n<|output|>"
        )
        return {
            "prompt": prompt,
            "temperature": temperature,
            "n_predict": 512,
            "stop": ["<|input|>"],
        }
    if request_shape == "openai":
        return {
            "model": model,
            "temperature": temperature,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": build_user_message(case)},
            ],
        }
    raise HarnessError("request_invalid", "request shape must be template, completion, or openai")


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
