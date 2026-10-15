"""Parsing of local, completion, and OpenAI-compatible extractor responses."""

from __future__ import annotations

import json
from typing import Any, Mapping

from extract_quality_core import MAX_RESPONSE_BYTES, Candidate, HarnessError

def _candidate_from_object(value: Mapping[str, Any]) -> Candidate:
    if "error" in value and value["error"] is not None:
        raise HarnessError("provider_error", "provider returned an error")
    snapshot_value = value.get("snapshot")
    if snapshot_value is None:
        snapshot = None
    elif isinstance(snapshot_value, Mapping):
        snapshot = dict(snapshot_value)
    else:
        raise HarnessError("malformed_response", "snapshot must be an object or null")
    gist_value = value.get("gist")
    if gist_value is not None and not isinstance(gist_value, str):
        raise HarnessError("malformed_response", "gist must be a string or null")
    source_value = value.get("source_ref")
    if source_value is not None and not isinstance(source_value, str):
        raise HarnessError("malformed_response", "source_ref must be a string or null")
    diagnostic = value.get("diagnostic")
    return Candidate(
        snapshot=snapshot,
        gist=gist_value,
        source_ref=source_value or "extract",
        diagnostic=diagnostic,
    )


def _candidate_from_content(content: Any) -> Candidate:
    if not isinstance(content, str):
        raise HarnessError("empty_output", "response has no string content")
    if len(content.encode("utf-8")) > MAX_RESPONSE_BYTES:
        raise HarnessError("response_too_large", "assistant content exceeds the size limit")
    if not content.strip():
        raise HarnessError("empty_output", "assistant content is empty")
    try:
        parsed_content = json.loads(content)
    except (json.JSONDecodeError, RecursionError) as exc:
        raise HarnessError("malformed_response", "assistant content is not strict JSON") from exc
    if not isinstance(parsed_content, Mapping):
        raise HarnessError("malformed_response", "assistant content is not a JSON object")
    if "snapshot" in parsed_content or any(
        key in parsed_content for key in ("gist", "source_ref", "diagnostic")
    ):
        return _candidate_from_object(parsed_content)
    # llama.cpp NuExtract returns the snapshot object directly in `content`.
    return Candidate(
        snapshot=dict(parsed_content),
        gist=None,
        source_ref="extract",
        diagnostic=None,
    )


def parse_candidate_response(value: Any) -> Candidate:
    """Parse local, completion, or OpenAI-compatible extractor responses."""

    if not isinstance(value, Mapping):
        raise HarnessError("malformed_response", "response is not a JSON object")
    if "error" in value and value["error"] is not None:
        raise HarnessError("provider_error", "provider returned an error")
    if "content" in value:
        return _candidate_from_content(value["content"])
    if "choices" in value:
        choices = value.get("choices")
        if not isinstance(choices, list) or not choices:
            raise HarnessError("malformed_response", "response has no choices")
        first = choices[0]
        if not isinstance(first, Mapping):
            raise HarnessError("malformed_response", "first choice is not an object")
        message = first.get("message")
        if isinstance(message, Mapping) and "content" in message:
            return _candidate_from_content(message["content"])
        if "text" in first:
            return _candidate_from_content(first["text"])
        raise HarnessError("malformed_response", "choice has no message content")
    return _candidate_from_object(value)
