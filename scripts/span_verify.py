#!/usr/bin/env python3
"""Reference span-grounding checks for extraction quality tooling.

The verifier treats each non-empty string, number, boolean, or date-like leaf
in a structured snapshot as a value that must occur in the source text. It is
intentionally small and standard-library-only. The production Rust verifier
remains the authority for admission; this module is a harness/reference check.

Entities are matched as contiguous substrings after whitespace normalization.
Callers that need numeric/date parsing should use the field-aware verifier in
``extract_quality.py``; this reference API preserves the original generic span
contract for small scripts and fixtures.
"""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from typing import Any

_WHITESPACE = re.compile(r"\s+")


@dataclass(frozen=True)
class SpanIssue:
    """One ungrounded snapshot leaf."""

    path: str
    value: str
    reason: str


@dataclass(frozen=True)
class VerifyResult:
    """Result of checking all snapshot leaves."""

    ok: bool
    issues: tuple[SpanIssue, ...]

    def as_dict(self) -> dict[str, Any]:
        """Return a JSON-compatible result."""

        return {
            "ok": self.ok,
            "issues": [
                {"path": issue.path, "value": issue.value, "reason": issue.reason}
                for issue in self.issues
            ],
        }


def _normalize(value: str) -> str:
    return _WHITESPACE.sub(" ", value.strip())


def iter_leaves(value: Any, path: str = "$") -> list[tuple[str, str]]:
    """Flatten JSON-like values into path/value pairs."""

    leaves: list[tuple[str, str]] = []
    if value is None:
        return leaves
    if isinstance(value, bool):
        leaves.append((path, "true" if value else "false"))
    elif isinstance(value, (int, float)):
        leaves.append((path, str(value)))
    elif isinstance(value, str):
        leaves.append((path, value))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            leaves.extend(iter_leaves(item, f"{path}[{index}]"))
    elif isinstance(value, dict):
        for key, item in value.items():
            leaves.extend(iter_leaves(item, f"{path}.{key}"))
    else:
        leaves.append((path, str(value)))
    return leaves


def verify_snapshot(source: str, snapshot: Any) -> VerifyResult:
    """Return a failure for every non-empty leaf absent from ``source``."""

    source_normalized = _normalize(source)
    issues: list[SpanIssue] = []
    for path, value in iter_leaves(snapshot):
        if not value or not _normalize(value):
            continue
        if _normalize(value) not in source_normalized and value not in source:
            issues.append(
                SpanIssue(
                    path=path,
                    value=value,
                    reason="not a contiguous source span",
                )
            )
    return VerifyResult(ok=not issues, issues=tuple(issues))


def parse_extract_output(raw: str) -> Any:
    """Parse a JSON object, tolerating only surrounding whitespace."""

    try:
        return json.loads(raw.strip())
    except json.JSONDecodeError as exc:
        raise ValueError("extractor output is not valid JSON") from exc


def _self_check() -> int:
    source = (
        "Build failed with E0382 on line 42. "
        "Rust 1.75.0. Timeout set to 30 seconds by Alice on 2026-09-16."
    )
    good = {
        "error": "E0382",
        "line": "42",
        "rust": "1.75.0",
        "timeout_seconds": "30",
        "owner": "Alice",
        "date": "2026-09-16",
        "missing": "",
    }
    bad = {"error": "E0382", "line": "99", "owner": "Bob"}
    assert verify_snapshot(source, good).ok
    result = verify_snapshot(source, bad)
    assert not result.ok and len(result.issues) == 2
    assert parse_extract_output('{"a":"1"}') == {"a": "1"}
    print("span_verify self-check: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(_self_check())
