"""Request construction for the live extraction harness."""

from __future__ import annotations

from typing import Any
from urllib.parse import urlparse, urlunparse

from extract_quality_core import FixtureCase, HarnessError
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
