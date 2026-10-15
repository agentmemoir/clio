"""Bounded, recursive redaction of every value the harness prints or stores."""

from __future__ import annotations

import json
import re
from typing import Any, Mapping

from extract_quality_core import MAX_RAW_ITEMS, MAX_RAW_STRING_CHARS

_SECRET_KEY = re.compile(
    r"(?:api[_-]?key|access[_-]?token|github[_-]?token|slack[_-]?token|"
    r"aws[_-]?access[_-]?key(?:[_-]?id)?|jwt|auth(?:orization)?|bearer|"
    r"password|passwd|secret|credential|private[_-]?key)",
    re.IGNORECASE,
)
_BEARER_VALUE_PATTERN = re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{8,255}")
_SECRET_VALUE_PATTERNS = (
    re.compile(r"\bsk-[A-Za-z0-9_-]{8,255}\b"),
    _BEARER_VALUE_PATTERN,
    re.compile(
        r"(?i)\b(api[_-]?key|access[_-]?token|github[_-]?token|"
        r"slack[_-]?token|aws[_-]?access[_-]?key(?:[_-]?id)?|jwt|"
        r"password|secret|token)\b\s*[:=]\s*[^\s,;]+"
    ),
    re.compile(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b"),
    re.compile(
        r"\b(?:gh[pousr]_[A-Za-z0-9_]{8,255}|"
        r"github_pat_[A-Za-z0-9_]{8,255})\b"
    ),
    re.compile(
        r"(?<![A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{1,2048}\."
        r"[A-Za-z0-9_-]{1,2048}\.[A-Za-z0-9_-]{1,2048}"
        r"(?![A-Za-z0-9_-])"
    ),
    re.compile(
        r"\b(?:xox(?:a|b|p|r|s)-[A-Za-z0-9_-]{8,255}|"
        r"xapp-\d-[A-Za-z0-9_-]{8,255}|"
        r"xoxe\.xox[baprs]-[A-Za-z0-9_-]{8,255})\b"
    ),
)


def _is_secret_key(key: str) -> bool:
    return bool(_SECRET_KEY.search(key))


def _redact_text(value: str, limit: int | None = MAX_RAW_STRING_CHARS) -> str:
    text = value
    for pattern in _SECRET_VALUE_PATTERNS:
        replacement = (
            "Bearer [REDACTED]"
            if pattern is _BEARER_VALUE_PATTERN
            else "[REDACTED]"
        )
        text = pattern.sub(replacement, text)
    if limit is not None and len(text) > limit:
        return text[:limit] + "[TRUNCATED]"
    return text


def redact_json(value: Any, key: str = "") -> Any:
    """Return a bounded, recursively redacted JSON-safe value."""

    if _is_secret_key(key):
        return "[REDACTED]"
    if isinstance(value, Mapping):
        result: dict[str, Any] = {}
        for index, (raw_key, raw_value) in enumerate(value.items()):
            if index >= MAX_RAW_ITEMS:
                result["[TRUNCATED]"] = "[TRUNCATED]"
                break
            key_text = str(raw_key)
            result[key_text] = redact_json(raw_value, key_text)
        return result
    if isinstance(value, (list, tuple)):
        result_list: list[Any] = []
        for index, item in enumerate(value):
            if index >= MAX_RAW_ITEMS:
                result_list.append("[TRUNCATED]")
                break
            result_list.append(redact_json(item))
        return result_list
    if isinstance(value, str):
        return _redact_text(value)
    if value is None or isinstance(value, (bool, int, float)):
        return value
    return _redact_text(str(value))


def _json_text(value: Any) -> str:
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )


def _looks_like_secret(value: str) -> bool:
    return any(pattern.search(value) for pattern in _SECRET_VALUE_PATTERNS)
