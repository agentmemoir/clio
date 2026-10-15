"""Evaluation and reporting of the shared verifier parity table."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Mapping

from extract_quality_core import EXIT_OK, EXIT_QUALITY_FAIL, HarnessError
from extract_quality_fixture import (
    _field_from_json,
    _load_json_file,
    _parse_verify_config,
)
from extract_quality_span import verify_snapshot

def _evaluate_parity(path: Path) -> tuple[str, list[dict[str, Any]]]:
    document = _load_json_file(path, "verifier parity table")
    if not isinstance(document, Mapping) or document.get("format") != "clio-verifier-parity-v1":
        raise HarnessError("parity_schema", "unsupported verifier parity format")
    cases = document.get("cases")
    if not isinstance(cases, list) or not cases:
        raise HarnessError("parity_schema", "verifier parity table has no cases")
    results: list[dict[str, Any]] = []
    seen: set[str] = set()
    for index, raw in enumerate(cases):
        if not isinstance(raw, Mapping):
            raise HarnessError("parity_schema", f"parity case {index} is not an object")
        case_id = raw.get("id")
        source_text = raw.get("source_text")
        snapshot = raw.get("snapshot")
        fields_value = raw.get("fields")
        expected = raw.get("expected_ok")
        if not isinstance(case_id, str) or not case_id:
            raise HarnessError("parity_schema", f"parity case {index} has no id")
        if case_id in seen:
            raise HarnessError("parity_schema", f"duplicate parity case id {case_id}")
        seen.add(case_id)
        if not isinstance(source_text, str):
            raise HarnessError("parity_schema", f"parity case {case_id} has no source_text")
        if not isinstance(fields_value, list) or not fields_value:
            raise HarnessError("parity_schema", f"parity case {case_id} has no fields")
        if not isinstance(expected, bool):
            raise HarnessError("parity_schema", f"parity case {case_id} has no expected_ok")
        fields = [_field_from_json(item, i) for i, item in enumerate(fields_value)]
        actual = verify_snapshot(
            source_text,
            snapshot,
            fields,
            _parse_verify_config(raw.get("verify")),
        ).ok
        results.append(
            {
                "id": case_id,
                "expected_ok": expected,
                "actual_ok": actual,
                "matches": actual == expected,
            }
        )
    return str(document["format"]), results


def _print_parity(path: Path) -> int:
    table_format, results = _evaluate_parity(path)
    failed = sum(1 for result in results if not result["matches"])
    payload = {
        "format": "clio-verifier-parity-result-v1",
        "table_format": table_format,
        "cases": results,
        "case_count": len(results),
        "passed": len(results) - failed,
        "failed": failed,
    }
    print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    return EXIT_QUALITY_FAIL if failed else EXIT_OK
