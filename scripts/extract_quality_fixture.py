"""Fixture loading and schema validation for the extraction harness."""

from __future__ import annotations

import json
import math
from pathlib import Path
from typing import Any, Mapping

from extract_quality_core import (
    MAX_FIXTURE_BYTES,
    FieldSpec,
    FixtureCase,
    FixtureSet,
    HarnessError,
    VerifyConfig,
)
from extract_quality_redact import _looks_like_secret
from extract_quality_span import verify_snapshot

def _read_bounded(path: Path, limit: int, label: str) -> bytes:
    try:
        with path.open("rb") as handle:
            data = handle.read(limit + 1)
    except OSError as exc:
        raise HarnessError("fixture_io", f"cannot read {label} {path.name}") from exc
    if len(data) > limit:
        raise HarnessError("fixture_too_large", f"{label} exceeds the size limit")
    return data


def _load_json_file(path: Path, label: str) -> Any:
    data = _read_bounded(path, MAX_FIXTURE_BYTES, label)
    try:
        return json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as exc:
        raise HarnessError("fixture_json", f"invalid JSON in {label} {path.name}") from exc


def _get_case_value(case: Mapping[str, Any], *names: str, default: Any = None) -> Any:
    for name in names:
        if name in case:
            return case[name]
    return default


def _schema_has_path(schema: Mapping[str, Any], path: str) -> bool:
    current: Any = schema
    for part in path.split("."):
        if not part:
            continue
        if not isinstance(current, Mapping) or part not in current:
            return False
        current = current[part]
    return True


def _field_from_json(value: Any, index: int) -> FieldSpec:
    if not isinstance(value, Mapping):
        raise HarnessError("fixture_schema", f"fields[{index}] is not an object")
    path = value.get("path")
    kind = value.get("kind", "entity")
    required = value.get("required", True)
    if not isinstance(path, str) or not path.strip():
        raise HarnessError("fixture_schema", f"fields[{index}] has no path")
    if kind not in {"entity", "number", "date"}:
        raise HarnessError("fixture_schema", f"fields[{index}] has an unknown kind")
    if not isinstance(required, bool):
        raise HarnessError("fixture_schema", f"fields[{index}].required is not boolean")
    return FieldSpec(path=path, kind=kind, required=required)


def _infer_fields(schema: Mapping[str, Any], expected: Mapping[str, Any]) -> list[FieldSpec]:
    fields: list[FieldSpec] = []
    keys = list(expected) or list(schema)
    for key in keys:
        lower = str(key).lower()
        if "date" in lower or lower in {"day", "when"}:
            kind = "date"
        elif any(token in lower for token in ("amount", "count", "number", "total", "quantity")):
            kind = "number"
        else:
            kind = "entity"
        required = expected.get(key) not in (None, "")
        fields.append(FieldSpec(path=str(key), kind=kind, required=required))
    return fields


def _parse_verify_config(value: Any) -> VerifyConfig:
    if value is None:
        return VerifyConfig()
    if not isinstance(value, Mapping):
        raise HarnessError("fixture_schema", "verify must be an object")
    day_first = value.get("day_first")
    if day_first is not None and not isinstance(day_first, bool):
        raise HarnessError("fixture_schema", "verify.day_first must be boolean or null")
    tolerance = value.get("number_tolerance", 0.0)
    if isinstance(tolerance, bool) or not isinstance(tolerance, (int, float)):
        raise HarnessError("fixture_schema", "verify.number_tolerance must be numeric")
    if not math.isfinite(float(tolerance)) or float(tolerance) < 0:
        raise HarnessError("fixture_schema", "verify.number_tolerance is invalid")
    grouping = value.get("comma_grouping", True)
    if not isinstance(grouping, bool):
        raise HarnessError("fixture_schema", "verify.comma_grouping must be boolean")
    return VerifyConfig(
        day_first=day_first,
        number_tolerance=float(tolerance),
        comma_grouping=grouping,
    )


def _parse_case(value: Any, index: int, suite_name: str) -> FixtureCase:
    if not isinstance(value, Mapping):
        raise HarnessError("fixture_schema", f"cases[{index}] is not an object")
    case_id = value.get("id")
    if not isinstance(case_id, str) or not case_id.strip():
        raise HarnessError("fixture_schema", f"cases[{index}] has no id")
    source_text = _get_case_value(value, "source_text", "text")
    if not isinstance(source_text, str) or not source_text:
        raise HarnessError("fixture_schema", f"case {case_id} has no source_text")
    if _looks_like_secret(source_text):
        raise HarnessError("fixture_secret", f"case {case_id} appears to contain a secret")
    schema = _get_case_value(value, "schema", "schema_template")
    if not isinstance(schema, Mapping):
        raise HarnessError("fixture_schema", f"case {case_id} has no schema object")
    expected = value.get("expected_snapshot")
    if not isinstance(expected, Mapping):
        raise HarnessError("fixture_schema", f"case {case_id} has no expected_snapshot object")
    gold_value = value.get("gold")
    if gold_value is None and suite_name != "contract" and expected:
        gold_value = {
            str(key): str(item)
            for key, item in expected.items()
            if isinstance(item, str)
            or (isinstance(item, (int, float)) and not isinstance(item, bool))
        }
    if gold_value is None:
        gold_value = {}
    if not isinstance(gold_value, Mapping) or not all(
        isinstance(key, str) and isinstance(item, str) for key, item in gold_value.items()
    ):
        raise HarnessError("fixture_schema", f"case {case_id} has an invalid gold object")
    baseline_raw = value.get("baseline_raw_response")
    raw_value = _get_case_value(value, "raw_response", "response", default=None)
    if raw_value is None and "raw_response" not in value and "response" not in value:
        raise HarnessError("fixture_schema", f"case {case_id} has no raw_response")
    expected_admission = value.get("expected_admission", not bool(value.get("control", False)))
    expected_grounding = value.get("expected_grounding", expected_admission)
    if not isinstance(expected_admission, bool) or not isinstance(expected_grounding, bool):
        raise HarnessError("fixture_schema", f"case {case_id} has non-boolean expectation")
    if expected_admission and not expected_grounding:
        raise HarnessError("fixture_schema", f"case {case_id} has inconsistent expectations")
    if expected_grounding and not expected_admission:
        raise HarnessError("fixture_schema", f"case {case_id} has inconsistent expectations")
    fields_value = value.get("fields")
    if fields_value is None:
        fields = _infer_fields(schema, expected)
    else:
        if not isinstance(fields_value, list) or not fields_value:
            raise HarnessError("fixture_schema", f"case {case_id} fields must be a nonempty list")
        fields = [_field_from_json(item, i) for i, item in enumerate(fields_value)]
    required_paths = {item.path for item in fields if item.required}
    if any(not _schema_has_path(schema, path) for path in required_paths):
        raise HarnessError("fixture_schema", f"case {case_id} schema omits a required field")
    expected_paths = {str(key) for key in expected}
    if not expected_paths.issubset({item.path for item in fields}):
        raise HarnessError("fixture_schema", f"case {case_id} expected field is not verified")
    if not set(gold_value).issubset(expected_paths):
        raise HarnessError("fixture_schema", f"case {case_id} gold field is not expected")
    expected_causes = value.get("expected_causes", [])
    if not isinstance(expected_causes, list) or not all(isinstance(x, str) for x in expected_causes):
        raise HarnessError("fixture_schema", f"case {case_id} expected_causes is invalid")
    case_class = value.get("class", value.get("case_class", "synthetic"))
    if not isinstance(case_class, str) or not case_class:
        raise HarnessError("fixture_schema", f"case {case_id} has an invalid class")
    control = value.get("control", not expected_admission or not expected_grounding)
    if not isinstance(control, bool):
        raise HarnessError("fixture_schema", f"case {case_id} control is not boolean")
    verify_feedback = value.get("verify_feedback")
    if verify_feedback is not None and not isinstance(verify_feedback, str):
        raise HarnessError("fixture_schema", f"case {case_id} verify_feedback is not a string")
    case = FixtureCase(
        case_id=case_id,
        case_class=case_class,
        source_text=source_text,
        schema=dict(schema),
        fields=fields,
        expected_snapshot=dict(expected),
        gold=dict(gold_value),
        expected_admission=expected_admission,
        expected_grounding=expected_grounding,
        raw_response=raw_value,
        baseline_raw_response=baseline_raw,
        control=control,
        expected_causes=list(expected_causes),
        tool_payload=value.get("tool_payload"),
        verify_feedback=verify_feedback,
        verify_config=_parse_verify_config(value.get("verify")),
    )
    expected_report = verify_snapshot(
        case.source_text,
        case.expected_snapshot,
        case.fields,
        case.verify_config,
    )
    if not expected_report.ok and not (
        case.control and "span_grounding" in case.expected_causes
    ):
        raise HarnessError("fixture_expected_ungrounded", f"expected snapshot for {case_id} is not grounded")
    return case


def load_fixture(path: Path, expected_set: str | None = None) -> FixtureSet:
    document = _load_json_file(path, "fixture set")
    if not isinstance(document, Mapping):
        raise HarnessError("fixture_schema", "fixture set is not an object")
    if document.get("format") != "clio-extract-fixture-v1":
        raise HarnessError("fixture_schema", "unsupported fixture format")
    if document.get("frozen") is not True:
        raise HarnessError("fixture_schema", "fixture set is not marked frozen")
    if not isinstance(document.get("construction"), str) or not document["construction"]:
        raise HarnessError("fixture_schema", "fixture set has no construction rule")
    set_name = document.get("set")
    if not isinstance(set_name, str) or not set_name:
        raise HarnessError("fixture_schema", "fixture set has no name")
    if expected_set is not None and set_name != expected_set:
        raise HarnessError("fixture_schema", f"expected {expected_set} set, found {set_name}")
    cases_value = document.get("cases")
    if not isinstance(cases_value, list) or not cases_value:
        raise HarnessError("fixture_schema", "fixture set has no cases")
    cases: list[FixtureCase] = []
    seen: set[str] = set()
    for index, value in enumerate(cases_value):
        case = _parse_case(value, index, set_name)
        if case.case_id in seen:
            raise HarnessError("fixture_schema", f"duplicate case id {case.case_id}")
        seen.add(case.case_id)
        cases.append(case)
    return FixtureSet(name=set_name, path=path, cases=cases)
