#!/usr/bin/env python3
"""Quality harness for extraction admission and span grounding.

The harness has two deliberately separate inputs:

* ``extract_fixtures.json`` is a frozen synthetic tuning set.  It also has
  small negative controls which preserve the recorded pre-fix empty response.
* ``extract_heldout.json`` is a separately frozen synthetic set.  Its expected
  values are authored from the construction grammar, never copied from a live
  response.

The default run executes the local unit set.  A live request is attempted only
when ``--endpoint`` is supplied.  Use ``--unit-only`` to make that intent
explicit, or ``--live-only`` to require a live run.  No live result is ever
synthesised when an endpoint is absent or unavailable.  ``--parity`` evaluates
``verifier_parity.json`` locally and emits per-case booleans without making a
request.

Fixture format (version 1)::

    {
      "format": "clio-extract-fixture-v1",
      "set": "tuning" | "heldout",
      "frozen": true,
      "construction": "why and when this synthetic set was frozen",
      "cases": [
        {
          "id": "unique-public-case-id",
          "class": "synthetic_class",
          "source_text": "synthetic source",
          "schema": {"field": ""},
          "fields": [
            {"path": "field", "kind": "entity|number|date", "required": true}
          ],
          "expected_snapshot": {"field": "exact value"},
          "gold": {"field": "exact value"},
          "expected_admission": true,
          "expected_grounding": true,
          "raw_response": {"snapshot": {}, "gist": "synthetic"},
          "baseline_raw_response": {"snapshot": {}}
        }
      ]
    }

``expected_admission`` and ``expected_grounding`` describe what the recorded
response must do.  Negative controls therefore retain a fixed expected
snapshot while asserting that their historical response is rejected.  The
span verifier is applied to every declared non-empty field, and non-empty
unlisted leaves are rejected fail-closed; an expected snapshot never makes an
ungrounded value acceptable.

Only Python's standard library is used.  Live HTTP is explicit, bounded,
time-limited, and never prints a response body.  ``--raw-output`` optionally
stores a redacted, bounded diagnostic record; it is not required for unit
mode.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import math
import os
import re
import socket
import sys
import unicodedata
from dataclasses import dataclass, field
from datetime import date, datetime
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse, urlunparse
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

EXIT_OK = 0
EXIT_QUALITY_FAIL = 1
EXIT_USAGE = 2
EXIT_FIXTURE_ERROR = 3

DEFAULT_MIN_FIDELITY = 0.99
DEFAULT_MODEL = "local-extract"
DEFAULT_TEMPERATURE = 0.0
DEFAULT_TIMEOUT = 10.0
MAX_TIMEOUT = 120.0
MAX_RESPONSE_BYTES = 1024 * 1024
MAX_REQUEST_BYTES = 2 * 1024 * 1024
MAX_FIXTURE_BYTES = 2 * 1024 * 1024
MAX_RAW_STRING_CHARS = 4096
MAX_RAW_ITEMS = 128

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))
from span_verify import verify_snapshot as verify_generic_snapshot  # noqa: E402

TUNING_FIXTURE = SCRIPT_DIR / "extract_fixtures.json"
HELDOUT_FIXTURE = SCRIPT_DIR / "extract_heldout.json"
VERIFIER_PARITY = SCRIPT_DIR / "verifier_parity.json"

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
_MONTH_NAME = (
    r"(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|"
    r"Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|"
    r"Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)"
)
_MONTH_DATE_RE = re.compile(
    rf"(?i)(?:{_MONTH_NAME}\s+\d{{1,2}}(?:,)?\s+\d{{4}}|"
    rf"\d{{1,2}}\s+{_MONTH_NAME}\s+\d{{4}})"
)


class HarnessError(Exception):
    """An input, configuration, or transport error with a stable cause."""

    def __init__(self, cause: str, message: str = "") -> None:
        self.cause = cause
        super().__init__(message or cause)


@dataclass(frozen=True)
class FieldSpec:
    path: str
    kind: str
    required: bool = True


@dataclass(frozen=True)
class VerifyConfig:
    day_first: bool | None = None
    number_tolerance: float = 0.0
    comma_grouping: bool = True


@dataclass
class VerifyReport:
    ok: bool
    spans: list[dict[str, Any]] = field(default_factory=list)
    failures: list[dict[str, str]] = field(default_factory=list)


@dataclass
class Candidate:
    snapshot: dict[str, Any] | None
    gist: str | None
    source_ref: str
    diagnostic: Any | None


@dataclass
class FixtureCase:
    case_id: str
    case_class: str
    source_text: str
    schema: dict[str, Any]
    fields: list[FieldSpec]
    expected_snapshot: dict[str, Any]
    expected_admission: bool
    expected_grounding: bool
    raw_response: Any
    control: bool
    expected_causes: list[str]
    tool_payload: Any | None
    verify_feedback: Any | None
    verify_config: VerifyConfig
    gold: dict[str, str] = field(default_factory=dict)
    baseline_raw_response: Any | None = None


@dataclass
class FixtureSet:
    name: str
    path: Path
    cases: list[FixtureCase]


@dataclass
class CaseResult:
    case_id: str
    suite: str
    expected_admission: bool
    expected_grounding: bool
    control: bool
    extracted: bool
    structural_admitted: bool
    admitted: bool
    grounded: bool
    exact: bool
    gold_hit: int
    gold_total: int
    success: bool
    cause: str | None
    failures: list[dict[str, str]] = field(default_factory=list)
    expected_failures: list[dict[str, str]] = field(default_factory=list)
    raw_record: dict[str, Any] | None = None


@dataclass
class SuiteReport:
    name: str
    results: list[CaseResult]
    fixture_count: int
    contract_count: int
    raw_records: list[dict[str, Any]] = field(default_factory=list)
    evaluated_fixture_count: int = 0
    skipped_fixture_cases: int = 0
    evidence: str = "quality"
    metric_label: str = "fidelity"

    @property
    def positive(self) -> list[CaseResult]:
        return [r for r in self.results if not r.control]

    @property
    def controls(self) -> list[CaseResult]:
        return [r for r in self.results if r.control]

    @property
    def extracted(self) -> int:
        return sum(1 for r in self.results if r.extracted)

    @property
    def admitted(self) -> int:
        return sum(1 for r in self.results if r.admitted)

    @property
    def structural_candidates(self) -> int:
        return sum(1 for r in self.results if r.structural_admitted)

    @property
    def grounded(self) -> int:
        return sum(1 for r in self.results if r.grounded)

    @property
    def observed_failures(self) -> int:
        return sum(
            1
            for r in self.results
            if (not r.admitted) or (not r.grounded) or (not r.control and not r.exact)
        )

    @property
    def failed(self) -> int:
        return sum(1 for r in self.results if not r.success)

    @property
    def causes(self) -> dict[str, int]:
        counts: dict[str, int] = {}
        for result in self.results:
            if result.cause:
                counts[result.cause] = counts.get(result.cause, 0) + 1
        return dict(sorted(counts.items()))

    @property
    def case_fidelity(self) -> float:
        positives = self.positive
        if not positives:
            return 0.0
        return sum(1 for r in positives if r.success) / len(positives)

    @property
    def gold_hit(self) -> int:
        return sum(r.gold_hit for r in self.positive)

    @property
    def gold_total(self) -> int:
        return sum(r.gold_total for r in self.positive)

    @property
    def gold_fidelity(self) -> float:
        return self.gold_hit / self.gold_total if self.gold_total else self.case_fidelity

    @property
    def fidelity(self) -> float:
        return self.gold_fidelity

    @property
    def grounding_fidelity(self) -> float:
        # Structural admission is deliberately separate from the authoritative
        # verifier gate.  The denominator includes every non-empty candidate,
        # including negative controls, so rejected candidates cannot disappear.
        candidates = [r for r in self.results if r.structural_admitted]
        if not candidates:
            return 0.0
        return sum(
            1 for r in candidates if r.admitted and r.grounded
        ) / len(candidates)

    def line(self) -> str:
        return (
            f"{self.name} evidence={self.evidence} cases={len(self.results)} "
            f"evaluated_fixture_cases={self.evaluated_fixture_count} "
            f"fixture_cases={self.fixture_count} skipped_fixture_cases={self.skipped_fixture_cases} "
            f"contracts={self.contract_count} extracted={self.extracted} "
            f"structural_candidates={self.structural_candidates} "
            f"admitted={self.admitted} grounded={self.grounded} failed={self.failed} "
            f"observed_failures={self.observed_failures} controls={len(self.controls)} "
            f"{self.metric_label}={self.fidelity:.4f} gold={self.gold_hit}/{self.gold_total} "
            f"case_fidelity={self.case_fidelity:.4f} "
            f"grounding_fidelity={self.grounding_fidelity:.4f} "
            f"causes={format_causes(self.causes)}"
        )


# ---------------------------------------------------------------------------
# Safe JSON and fixture handling


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


# ---------------------------------------------------------------------------
# Span verification: a stdlib implementation of the repository contract


def nfc(value: str) -> str:
    return unicodedata.normalize("NFC", value)


def json_path(root: Any, path: str) -> Any | None:
    current = root
    for part in path.split("."):
        if not part:
            continue
        if isinstance(current, Mapping):
            if part not in current:
                return None
            current = current[part]
        elif isinstance(current, list) and part.isdigit():
            index = int(part)
            if index >= len(current):
                return None
            current = current[index]
        else:
            return None
    return current


def _entity_span(source: str, value: Any) -> tuple[int, int]:
    if not isinstance(value, str):
        raise ValueError("entity field must be a string")
    needle = nfc(value).strip()
    if not needle:
        raise ValueError("empty entity value")
    start = source.find(needle)
    if start < 0:
        raise ValueError("entity not found as contiguous source span")
    return start, start + len(needle)


def _parse_number(raw: str, comma_grouping: bool) -> float | None:
    text = raw.strip()
    for currency in "$€£¥":
        text = text.replace(currency, "")
    text = text.replace(" ", "")
    if not text:
        return None
    if comma_grouping:
        text = text.replace(",", "")
    else:
        text = text.replace(".", "").replace(",", ".")
    try:
        value = float(text)
    except ValueError:
        return None
    return value if math.isfinite(value) else None


def _number_span(source: str, value: Any, config: VerifyConfig) -> tuple[int, int]:
    if isinstance(value, bool):
        raise ValueError("number field must be a number or numeric string")
    if isinstance(value, (int, float)):
        expected = float(value)
    elif isinstance(value, str):
        expected = _parse_number(value, True)
        if expected is None:
            raise ValueError("number field string did not parse")
    else:
        raise ValueError("number field must be a number or numeric string")
    if not math.isfinite(expected):
        raise ValueError("number field is not finite")

    chars = list(source)
    index = 0
    while index < len(chars):
        char = chars[index]
        if not (char.isascii() and char.isdigit()) and char not in "$€£¥+-":
            index += 1
            continue
        end = index
        while end < len(chars) and chars[end] in "$€£¥":
            end += 1
        if end < len(chars) and chars[end] in "+-":
            end += 1
        while end < len(chars) and chars[end] in "$€£¥":
            end += 1
        digit_start = end
        while end < len(chars) and (chars[end].isascii() and chars[end].isdigit() or chars[end] in ",."):
            end += 1
        if end > digit_start:
            parsed = _parse_number("".join(chars[index:end]), config.comma_grouping)
            if parsed is not None and abs(parsed - expected) <= config.number_tolerance:
                return index, end
        # Match the verifier's fail-closed scan: retry at the next character
        # after a token that did not parse or did not equal the value.
        index += 1
    raise ValueError("no locatable number span parses to snapshot value")


def _date_value(value: Any) -> tuple[date, tuple[int, int, int] | None]:
    if not isinstance(value, str) or not value.strip():
        raise ValueError("date field must be a non-empty string")
    text = value.strip()
    # Keep the snapshot grammar aligned with the production verifier: the two
    # accepted date-time separators are ``T`` and a single space in the value;
    # source tokenization below is intentionally stricter for space-separated
    # source spans (the production token alphabet does not contain spaces).
    for format_string in ("%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"):
        try:
            parsed = datetime.strptime(text, format_string)
            return parsed.date(), (parsed.hour, parsed.minute, parsed.second)
        except ValueError:
            continue
    try:
        return datetime.strptime(text, "%Y-%m-%d").date(), None
    except ValueError as exc:
        raise ValueError("snapshot date must be ISO date or date-time") from exc


def _numeric_date(token: str) -> tuple[int, int, int] | None:
    separator = "/" if "/" in token else "." if token.count(".") >= 2 and "-" not in token else ""
    if not separator:
        return None
    parts = token.split(separator)
    if len(parts) != 3:
        return None
    try:
        first, second, year = (int(part) for part in parts)
    except ValueError:
        return None
    if year < 100:
        return None
    return first, second, year


def _safe_date(year: int, month: int, day: int) -> date | None:
    try:
        return date(year, month, day)
    except ValueError:
        return None


def _try_date_token(
    token: str,
    expected: tuple[date, tuple[int, int, int] | None],
    config: VerifyConfig,
) -> tuple[bool, bool]:
    """Return ``(matched, ambiguous)`` for one production-shaped token."""

    expected_date, expected_time = expected
    parsed_date: date | None = None
    parsed_time: tuple[int, int, int] | None = None
    try:
        if "T" in token:
            parsed = datetime.strptime(token, "%Y-%m-%dT%H:%M:%S")
            parsed_date = parsed.date()
            parsed_time = (parsed.hour, parsed.minute, parsed.second)
        else:
            parsed_date = datetime.strptime(token, "%Y-%m-%d").date()
    except ValueError:
        numeric = _numeric_date(token)
        if numeric is None:
            return False, False
        first, second, year = numeric
        # A component greater than twelve identifies the other component as
        # the day, so only the valid calendar interpretation is considered.
        if first > 12 and second <= 12:
            parsed_date = _safe_date(year, second, first)
        elif second > 12 and first <= 12:
            parsed_date = _safe_date(year, first, second)
        elif first <= 12 and second <= 12:
            day_first = _safe_date(year, second, first)
            month_first = _safe_date(year, first, second)
            possible = [
                item for item in (day_first, month_first) if item is not None
            ]
            if not possible:
                return False, False
            matches = expected_time is None and expected_date in possible
            if config.day_first is None:
                # Rust's verifier reports ambiguity only when both calendar
                # interpretations exist and one could match the snapshot.
                return False, len(possible) == 2 and matches
            parsed_date = day_first if config.day_first else month_first
        else:
            return False, False

    if parsed_date != expected_date:
        return False, False
    # A date-only snapshot can ground against a source date-time, but a
    # snapshot carrying a time requires the same time in the source.
    if expected_time is not None and parsed_time != expected_time:
        return False, False
    return True, False


def _month_date(token: str) -> date | None:
    normalized = re.sub(r"\s+", " ", token.strip()).replace(" ,", ",")
    for format_string in ("%b %d, %Y", "%B %d, %Y", "%d %b %Y", "%d %B %Y"):
        try:
            return datetime.strptime(normalized, format_string).date()
        except ValueError:
            continue
    return None


def _date_span(source: str, value: Any, config: VerifyConfig) -> tuple[int, int]:
    expected_date, expected_time = _date_value(value)
    for match in _MONTH_DATE_RE.finditer(source):
        parsed = _month_date(match.group(0))
        if parsed == expected_date and expected_time is None:
            return match.start(), match.end()
    expected = (expected_date, expected_time)
    chars = list(source)
    saw_ambiguous = False
    index = 0
    while index < len(chars):
        if chars[index].isascii() and chars[index].isdigit():
            end = index
            while end < len(chars) and (
                (chars[end].isascii() and chars[end].isdigit()) or chars[end] in "-/.:T"
            ):
                end += 1
            # Production tokenization requires a date-like token and does not
            # include spaces in its alphabet.  A space therefore terminates
            # the token, just as it does in the Rust verifier.  Punctuation is
            # deliberately not trimmed here: the reference follows the
            # authoritative scanner exactly.
            if end > index + 4:
                token = "".join(chars[index:end])
                matched, ambiguous = _try_date_token(token, expected, config)
                if matched:
                    return index, end
                saw_ambiguous = saw_ambiguous or ambiguous
                index = end
            else:
                index += 1
        else:
            index += 1
    if saw_ambiguous:
        raise ValueError("ambiguous date without day_first policy")
    raise ValueError("no locatable date span parses to snapshot value")


def _iter_snapshot_leaves(value: Any, path: str = "") -> Iterable[tuple[str, Any]]:
    if isinstance(value, Mapping):
        for raw_key, item in value.items():
            key = str(raw_key)
            child = f"{path}.{key}" if path else key
            yield from _iter_snapshot_leaves(item, child)
    elif isinstance(value, (list, tuple)):
        for index, item in enumerate(value):
            child = f"{path}.{index}" if path else str(index)
            yield from _iter_snapshot_leaves(item, child)
    else:
        yield path, value


def _is_empty_snapshot_leaf(value: Any) -> bool:
    return value is None or (isinstance(value, str) and not value.strip())


def verify_snapshot(
    source_text: str,
    snapshot: Any,
    fields: Sequence[FieldSpec],
    config: VerifyConfig | None = None,
) -> VerifyReport:
    config = config or VerifyConfig()
    source = nfc(source_text)
    if not isinstance(snapshot, Mapping):
        return VerifyReport(False, [], [{"field": "<snapshot>", "reason": "snapshot is not an object"}])
    spans: list[dict[str, Any]] = []
    failures: list[dict[str, str]] = []
    for spec in fields:
        value = json_path(snapshot, spec.path)
        missing = value is None or value == "" or (isinstance(value, str) and not value.strip())
        if missing:
            if spec.required:
                failures.append({"field": spec.path, "reason": "missing required extractive field"})
            continue
        try:
            if spec.kind == "entity":
                start, end = _entity_span(source, value)
            elif spec.kind == "number":
                start, end = _number_span(source, value, config)
            else:
                start, end = _date_span(source, value, config)
        except ValueError as exc:
            failures.append({"field": spec.path, "reason": str(exc)})
        else:
            spans.append({"field": spec.path, "start": start, "end": end})

    # A declared field has a typed extraction policy.  There is no safe typed
    # policy for an unlisted leaf, so reject every non-empty one fail-closed
    # instead of allowing schema-shaped metadata to bypass verification.
    # Snapshot keys are caller-controlled, so the failure names the walk
    # position only; the key and path are never echoed (see verify.rs).
    declared_paths = {spec.path for spec in fields}
    unlisted = 0
    for path, value in _iter_snapshot_leaves(snapshot):
        if path in declared_paths or _is_empty_snapshot_leaf(value):
            continue
        failures.append(
            {
                "field": f"<unlisted leaf #{unlisted}>",
                "reason": "unlisted snapshot leaf rejected",
            }
        )
        unlisted += 1
    return VerifyReport(not failures, spans, failures)


def _snapshot_equal(
    actual: Any,
    expected: Any,
    fields: Sequence[FieldSpec] | None = None,
    prefix: str = "",
) -> bool:
    if isinstance(actual, Mapping) and isinstance(expected, Mapping):
        for key, expected_value in expected.items():
            if key not in actual:
                return False
            path = f"{prefix}.{key}" if prefix else str(key)
            if not _snapshot_equal(actual[key], expected_value, fields, path):
                return False
        for key, actual_value in actual.items():
            if key in expected:
                continue
            path = f"{prefix}.{key}" if prefix else str(key)
            spec = next((item for item in (fields or []) if item.path == path), None)
            if not (spec and not spec.required and actual_value in (None, "")):
                return False
        return True
    if isinstance(actual, list) and isinstance(expected, list):
        return len(actual) == len(expected) and all(
            _snapshot_equal(a, b, fields, prefix) for a, b in zip(actual, expected)
        )
    if isinstance(actual, str) and isinstance(expected, str):
        return nfc(actual) == nfc(expected)
    if isinstance(actual, (int, float)) and not isinstance(actual, bool) and isinstance(expected, (int, float)) and not isinstance(expected, bool):
        return math.isclose(float(actual), float(expected), rel_tol=0.0, abs_tol=0.0)
    return actual == expected


# ---------------------------------------------------------------------------
# Candidate response parsing


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


# ---------------------------------------------------------------------------
# Unit evaluation


def _gold_hits(snapshot: Any, gold: Mapping[str, str]) -> int:
    if not isinstance(snapshot, Mapping):
        return 0
    hits = 0
    for key, wanted in gold.items():
        actual = snapshot.get(key, "")
        if isinstance(actual, (int, float)) and not isinstance(actual, bool):
            actual = str(actual)
        if isinstance(actual, str) and (actual == wanted or wanted in actual):
            hits += 1
    return hits


def _case_result(case: FixtureCase, suite: str, raw_response: Any) -> CaseResult:
    failures: list[dict[str, str]] = []
    extracted = False
    structural_admitted = False
    admitted = False
    grounded = False
    exact = False
    gold_hit = 0
    gold_total = 0 if case.control else len(case.gold)
    cause: str | None = None
    try:
        candidate = parse_candidate_response(raw_response)
        if not candidate.snapshot:
            if not case.gold and not case.expected_snapshot:
                admitted = True
                grounded = True
                exact = True
            else:
                cause = "empty_extract"
        else:
            extracted = True
            # A non-empty, structurally parseable snapshot is a candidate even
            # when the authoritative span verifier will reject it below.
            structural_admitted = True
            verify = verify_snapshot(
                case.source_text,
                candidate.snapshot,
                case.fields,
                case.verify_config,
            )
            grounded = verify.ok
            failures = verify.failures
            if not grounded:
                cause = "span_grounding"
            else:
                # Admission in this harness means that the authoritative
                # verifier accepted the candidate.  Exact expected-snapshot
                # comparison is a separate fidelity check.
                admitted = True
                exact = _snapshot_equal(candidate.snapshot, case.expected_snapshot, case.fields)
                gold_hit = _gold_hits(candidate.snapshot, case.gold)
                if not exact:
                    cause = "snapshot_mismatch"
    except HarnessError as exc:
        cause = exc.cause
    expected_failures: list[dict[str, str]] = []
    if case.control:
        success = not admitted and not grounded
        if success and case.expected_causes and cause not in case.expected_causes:
            success = False
            expected_failures.append(
                {"field": "<control>", "reason": f"expected one of {sorted(case.expected_causes)}"}
            )
    else:
        success = admitted and grounded and exact
    if success and not case.control and case.expected_causes and cause in case.expected_causes:
        # A positive case should not silently carry a known failure cause.
        success = False
        expected_failures.append({"field": "<case>", "reason": "unexpected failure cause"})
    if case.expected_admission and not admitted:
        expected_failures.append({"field": "<admission>", "reason": "expected admission"})
    if case.expected_grounding and not grounded:
        expected_failures.append({"field": "<grounding>", "reason": "expected grounding"})
    if success and expected_failures:
        success = False
    return CaseResult(
        case_id=case.case_id,
        suite=suite,
        expected_admission=case.expected_admission,
        expected_grounding=case.expected_grounding,
        control=case.control,
        extracted=extracted,
        structural_admitted=structural_admitted,
        admitted=admitted,
        grounded=grounded,
        exact=exact,
        gold_hit=gold_hit,
        gold_total=gold_total,
        success=success,
        cause=cause,
        failures=failures,
        expected_failures=expected_failures,
    )


def _contract_cases() -> list[FixtureCase]:
    """Deterministic tests for the Python/span-verifier contract."""

    return [
        FixtureCase(
            case_id="contract-grounded-entity-number-date",
            case_class="contract_grounded",
            source_text="Mara Quill shipped Northstar 2 on 2025-01-02",
            schema={"entity": "", "amount": "", "date": ""},
            fields=[
                FieldSpec("entity", "entity", True),
                FieldSpec("amount", "number", True),
                FieldSpec("date", "date", True),
            ],
            expected_snapshot={"entity": "Mara Quill", "amount": 2, "date": "2025-01-02"},
            expected_admission=True,
            expected_grounding=True,
            raw_response={
                "snapshot": {"entity": "Mara Quill", "amount": 2, "date": "2025-01-02"},
                "gist": "Mara Quill shipped Northstar 2.",
                "source_ref": "contract",
            },
            control=False,
            expected_causes=[],
            tool_payload=None,
            verify_feedback=None,
            verify_config=VerifyConfig(),
        ),
        FixtureCase(
            case_id="contract-paraphrase-rejected",
            case_class="contract_rejection",
            source_text="Mara Quill prefers concise replies.",
            schema={"entity": ""},
            fields=[FieldSpec("entity", "entity", True)],
            expected_snapshot={"entity": "Mara Quill"},
            expected_admission=False,
            expected_grounding=False,
            raw_response={"snapshot": {"entity": "The user"}},
            control=True,
            expected_causes=["span_grounding"],
            tool_payload=None,
            verify_feedback=None,
            verify_config=VerifyConfig(),
        ),
        FixtureCase(
            case_id="contract-missing-required-rejected",
            case_class="contract_rejection",
            source_text="Northstar is ready.",
            schema={"owner": ""},
            fields=[FieldSpec("owner", "entity", True)],
            expected_snapshot={"owner": "Rin Sol"},
            expected_admission=False,
            expected_grounding=False,
            raw_response={"snapshot": {"status": "ready"}},
            control=True,
            expected_causes=["span_grounding"],
            tool_payload=None,
            verify_feedback=None,
            verify_config=VerifyConfig(),
        ),
        FixtureCase(
            case_id="contract-ambiguous-date-rejected",
            case_class="contract_rejection",
            source_text="The check ran on 03/04/2026",
            schema={"date": ""},
            fields=[FieldSpec("date", "date", True)],
            expected_snapshot={"date": "2026-04-03"},
            expected_admission=False,
            expected_grounding=False,
            raw_response={"snapshot": {"date": "2026-04-03"}},
            control=True,
            expected_causes=["span_grounding"],
            tool_payload=None,
            verify_feedback=None,
            verify_config=VerifyConfig(),
        ),
        FixtureCase(
            case_id="contract-optional-field-accepted",
            case_class="contract_grounded",
            source_text="Rin Sol owns the Lantern release on 2025-08-22.",
            schema={"owner": "", "ticket": ""},
            fields=[
                FieldSpec("owner", "entity", True),
                FieldSpec("ticket", "entity", False),
            ],
            expected_snapshot={"owner": "Rin Sol"},
            expected_admission=True,
            expected_grounding=True,
            raw_response={"snapshot": {"owner": "Rin Sol"}},
            control=False,
            expected_causes=[],
            tool_payload=None,
            verify_feedback=None,
            verify_config=VerifyConfig(),
        ),
    ]


def _evaluate_cases(
    cases: Iterable[FixtureCase], suite: str, historical: bool = False
) -> list[CaseResult]:
    results: list[CaseResult] = []
    for case in cases:
        response = case.baseline_raw_response if historical and case.baseline_raw_response is not None else case.raw_response
        results.append(_case_result(case, suite, response))
    return results


def run_unit(sets: Sequence[FixtureSet], historical: bool = False) -> SuiteReport:
    results: list[CaseResult] = []
    fixture_count = 0
    for fixture_set in sets:
        results.extend(
            _evaluate_cases(fixture_set.cases, f"unit:{fixture_set.name}", historical)
        )
        fixture_count += len(fixture_set.cases)
    contract = _contract_cases()
    results.extend(_evaluate_cases(contract, "unit:contract", historical))
    if not results:
        raise HarnessError("fixture_schema", "unit set has no cases")
    return SuiteReport(
        name="unit",
        results=results,
        fixture_count=fixture_count,
        contract_count=len(contract),
        evaluated_fixture_count=fixture_count,
        evidence="historical_fixture_replay" if historical else "fixture_contract_replay",
        metric_label="fixture_replay_fidelity" if historical else "fixture_contract_fidelity",
    )


# ---------------------------------------------------------------------------
# Live request/response handling


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


# ---------------------------------------------------------------------------
# Self-test and command line


def format_causes(causes: Mapping[str, int]) -> str:
    if not causes:
        return "none"
    return ",".join(f"{key}={value}" for key, value in sorted(causes.items()))


def _aggregate(reports: Sequence[SuiteReport]) -> SuiteReport:
    results = [result for report in reports for result in report.results]
    evidence_values = {report.evidence for report in reports}
    metric_values = {report.metric_label for report in reports}
    return SuiteReport(
        name="total",
        results=results,
        fixture_count=sum(report.fixture_count for report in reports),
        contract_count=sum(report.contract_count for report in reports),
        evaluated_fixture_count=sum(report.evaluated_fixture_count for report in reports),
        skipped_fixture_cases=sum(report.skipped_fixture_cases for report in reports),
        evidence=next(iter(evidence_values)) if len(evidence_values) == 1 else "mixed_evidence",
        metric_label=next(iter(metric_values)) if len(metric_values) == 1 else "quality_fidelity",
    )


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


def _self_test() -> int:
    checks: list[tuple[str, bool]] = []

    def check(name: str, condition: bool) -> None:
        checks.append((name, bool(condition)))

    grounded = verify_snapshot(
        "Mara Quill paid 1,024 credits on 2025-02-14",
        {"entity": "Mara Quill", "amount": 1024, "date": "2025-02-14"},
        [FieldSpec("entity", "entity"), FieldSpec("amount", "number"), FieldSpec("date", "date")],
    )
    check("grounded entity/number/date", grounded.ok and len(grounded.spans) == 3)
    check("generic reference verifier agrees", verify_generic_snapshot(
        "Mara Quill paid 1024 credits on 2025-02-14",
        {"entity": "Mara Quill", "amount": 1024, "date": "2025-02-14"},
    ).ok)
    bad = verify_snapshot(
        "Mara Quill paid 1,024 credits on 2025-02-14",
        {"entity": "The user", "amount": 1024, "date": "2025-02-14"},
        [FieldSpec("entity", "entity")],
    )
    check("paraphrase rejected", not bad.ok)
    ambiguous = verify_snapshot(
        "The check ran on 03/04/2026",
        {"date": "2026-04-03"},
        [FieldSpec("date", "date")],
    )
    check("ambiguous date rejected", not ambiguous.ok)
    nonmatching_ambiguous = verify_snapshot(
        "The check ran on 03/04/2026",
        {"date": "2026-05-03"},
        [FieldSpec("date", "date")],
    )
    check(
        "nonmatching numeric date is not called ambiguous",
        not any("ambiguous" in failure["reason"] for failure in nonmatching_ambiguous.failures),
    )
    invalid_numeric = verify_snapshot(
        "The check ran on 31/04/2026",
        {"date": "2026-04-30"},
        [FieldSpec("date", "date")],
    )
    check(
        "invalid numeric interpretation is not called ambiguous",
        not any("ambiguous" in failure["reason"] for failure in invalid_numeric.failures),
    )
    unambiguous_day = verify_snapshot(
        "The check ran on 25/04/2026",
        {"date": "2026-04-25"},
        [FieldSpec("date", "date")],
    )
    unambiguous_month = verify_snapshot(
        "The check ran on 04/25/2026",
        {"date": "2026-04-25"},
        [FieldSpec("date", "date")],
    )
    check(
        "unambiguous numeric dates accepted",
        unambiguous_day.ok and unambiguous_month.ok,
    )
    day_first = verify_snapshot(
        "The check ran on 03/04/2026",
        {"date": "2026-04-03"},
        [FieldSpec("date", "date")],
        VerifyConfig(day_first=True),
    )
    check("configured day-first date accepted", day_first.ok)
    t_date_time = verify_snapshot(
        "The audit is scheduled for 2026-01-09T14:30:00",
        {"date": "2026-01-09T14:30:00"},
        [FieldSpec("date", "date")],
    )
    check("T-separated source date-time accepted", t_date_time.ok)
    space_date_time = verify_snapshot(
        "The audit is scheduled for 2026-01-09 14:30:00",
        {"date": "2026-01-09 14:30:00"},
        [FieldSpec("date", "date")],
    )
    check("space-separated source date-time rejected", not space_date_time.ok)
    wrong_time = verify_snapshot(
        "The audit is scheduled for 2026-01-09T14:30:00",
        {"date": "2026-01-09T14:31:00"},
        [FieldSpec("date", "date")],
    )
    check("date-time mismatch rejected", not wrong_time.ok)
    invented_leaf = verify_snapshot(
        "Mara Quill is ready.",
        {"entity": "Mara Quill", "invented": "ready"},
        [FieldSpec("entity", "entity", True)],
    )
    check("unlisted non-empty leaf rejected", not invented_leaf.ok)
    invented_result = _case_result(
        _contract_cases()[0],
        "self-test",
        {"snapshot": {"entity": "Mara Quill", "invented": "Northstar"}},
    )
    invented_report = SuiteReport(
        name="self-test",
        results=[invented_result],
        fixture_count=0,
        contract_count=1,
    )
    check(
        "unlisted leaf cannot reach authoritative admission",
        invented_result.structural_admitted
        and not invented_result.admitted
        and invented_report.grounding_fidelity < 1.0,
    )
    local = parse_candidate_response({"snapshot": {"entity": "Mara Quill"}})
    check("local response parsed", local.snapshot == {"entity": "Mara Quill"})
    openai = parse_candidate_response(
        {"choices": [{"message": {"content": '{"snapshot":{"entity":"Mara Quill"}}'}}]}
    )
    check("OpenAI response parsed", openai.snapshot == {"entity": "Mara Quill"})
    completion = parse_candidate_response(
        {"content": '{"snapshot":{"entity":"Mara Quill"}}'}
    )
    check("completion response parsed", completion.snapshot == {"entity": "Mara Quill"})
    direct_completion = parse_candidate_response(
        {"content": '{"rust_version":"1.75.0","os":"ubuntu-22.04"}'}
    )
    check(
        "direct completion snapshot parsed",
        direct_completion.snapshot
        == {"rust_version": "1.75.0", "os": "ubuntu-22.04"},
    )
    malformed = False
    try:
        parse_candidate_response({"choices": [{"message": {"content": "```json\\n{}\\n```"}}]})
    except HarnessError as exc:
        malformed = exc.cause == "malformed_response"
    check("fenced content rejected", malformed)
    request = build_request(
        _contract_cases()[0], "test-model", 0.0, "template"
    )
    check(
        "local request shape",
        set(request) == {"model", "temperature", "template", "text", "tool_payload", "verify_feedback"},
    )
    completion_request = build_request(
        _contract_cases()[0], "test-model", 0.0, "completion"
    )
    check(
        "completion request includes recall instructions",
        "### Instructions:" in completion_request["prompt"]
        and "abstract label" in completion_request["prompt"],
    )
    check(
        "prompt requires all fields and exact spans",
        "every field" in SYSTEM_PROMPT and "exact source span" in SYSTEM_PROMPT,
    )
    redacted = redact_json({"api_key": "example-credential", "nested": {"password": "abc"}})
    check(
        "raw secrets redacted",
        "example-credential" not in _json_text(redacted) and "abc" not in _json_text(redacted),
    )
    secret_samples = (
        ("AWS access-key id", "AKIAIOSFODNN7EXAMPLE"),
        ("GitHub token", "ghp_0123456789abcdefghijklmnopqrstuvwxyz"),
        ("JWT", "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.signature"),
        # Split so the file text holds no scannable token; the sample is the
        # same string at run time.
        ("Slack token", "xox" + "b-123456789012-123456789012-abcdefghijklmnop"),
    )
    for label, sample in secret_samples:
        rendered = _redact_text(sample)
        check(f"{label} redacted", sample not in rendered)
    try:
        _table_format, parity_results = _evaluate_parity(VERIFIER_PARITY)
        check(
            "shared verifier parity table matches",
            bool(parity_results) and all(result["matches"] for result in parity_results),
        )
    except HarnessError:
        check("shared verifier parity table matches", False)
    failures = sum(1 for _, passed in checks if not passed)
    for name, passed in checks:
        print(f"extract-quality self-test {'ok' if passed else 'FAIL'}: {name}")
    print(f"extract-quality self-test: {len(checks) - failures} passed, {failures} failed")
    return EXIT_OK if failures == 0 else EXIT_QUALITY_FAIL


def _float_arg(value: str) -> float:
    try:
        parsed = float(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a number") from exc
    if not math.isfinite(parsed) or not 0.0 <= parsed <= 1.0:
        raise argparse.ArgumentTypeError("must be finite and between 0 and 1")
    return parsed


def _timeout_arg(value: str) -> float:
    try:
        parsed = float(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a number") from exc
    if not math.isfinite(parsed) or not 0.1 <= parsed <= MAX_TIMEOUT:
        raise argparse.ArgumentTypeError(f"must be between 0.1 and {MAX_TIMEOUT:g}")
    return parsed


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Evaluate fixture replay or an explicit provider run at the authoritative span boundary."
    )
    parser.add_argument(
        "--min-fidelity",
        type=_float_arg,
        default=DEFAULT_MIN_FIDELITY,
        help="minimum positive-case fixture or live fidelity (default: 0.99)",
    )
    parser.add_argument("--unit-only", action="store_true", help="run fixtures and contracts; never make HTTP requests")
    parser.add_argument("--live-only", action="store_true", help="run only explicit live requests; requires --endpoint")
    parser.add_argument(
        "--endpoint",
        "--live-endpoint",
        "--extract-url",
        dest="endpoint",
        help="explicit http(s) extractor endpoint for live mode",
    )
    parser.add_argument("--model", default=DEFAULT_MODEL, help="model id sent in a live request")
    parser.add_argument("--temperature", type=float, default=DEFAULT_TEMPERATURE, help="live temperature")
    parser.add_argument("--timeout", type=_timeout_arg, default=DEFAULT_TIMEOUT, help="live timeout in seconds")
    parser.add_argument(
        "--request-shape",
        choices=("template", "completion", "openai"),
        default="template",
        help="request wire shape: repository template, llama.cpp completion, or OpenAI chat",
    )
    parser.add_argument(
        "--historical",
        "--baseline",
        action="store_true",
        help="evaluate recorded pre-fix responses instead of fixed fixture responses",
    )
    parser.add_argument(
        "--no-retry",
        action="store_true",
        help="send one request per live case instead of the bounded retry",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print a compact machine-readable summary after the human report",
    )
    parser.add_argument(
        "--set",
        dest="fixture_set",
        choices=("tuning", "heldout", "both"),
        default="tuning",
        help="fixture set to use (default: tuning; heldout is explicit)",
    )
    parser.add_argument(
        "--heldout",
        action="store_true",
        help="shortcut for --set heldout",
    )
    parser.add_argument(
        "--fixtures",
        type=Path,
        help="override the selected fixture file; cannot be used with --set both",
    )
    parser.add_argument(
        "--raw-output",
        type=Path,
        help="write a bounded, redacted live diagnostic record to this path",
    )
    parser.add_argument("--self-test", action="store_true", help="run deterministic harness contract checks")
    parser.add_argument(
        "--parity",
        nargs="?",
        const=VERIFIER_PARITY,
        type=Path,
        metavar="PATH",
        help="evaluate the shared verifier parity table and emit per-case JSON booleans",
    )
    return parser


def _load_selected_sets(selection: str, override: Path | None) -> list[FixtureSet]:
    if override is not None:
        if selection == "both":
            raise HarnessError("fixture_selection", "--fixtures cannot be combined with --set both")
        expected = "tuning" if selection == "tuning" else "heldout"
        return [load_fixture(override, expected)]
    if selection == "tuning":
        return [load_fixture(TUNING_FIXTURE, "tuning")]
    if selection == "heldout":
        return [load_fixture(HELDOUT_FIXTURE, "heldout")]
    return [load_fixture(TUNING_FIXTURE, "tuning"), load_fixture(HELDOUT_FIXTURE, "heldout")]


def _print_report(report: SuiteReport, threshold: float) -> None:
    print("extract-quality: " + report.line())
    observed = [
        result
        for result in report.results
        if (not result.admitted)
        or (not result.grounded)
        or (not result.control and not result.exact)
    ]
    if observed:
        observations = ",".join(
            f"{_redact_text(result.case_id)}:{_redact_text(result.cause or 'unknown')}"
            for result in observed
        )
        print(f"extract-quality: observed rejected cases={observations}")
    if report.failed:
        for result in report.results:
            if not result.success:
                safe_case = _redact_text(result.case_id)
                detail = _redact_text(result.cause or "unknown")
                print(f"extract-quality: FAIL case={safe_case} cause={detail}")
                for failure in result.failures + result.expected_failures:
                    print(
                        "extract-quality: "
                        f"detail case={safe_case} "
                        f"field={_redact_text(str(failure.get('field', '?')))} "
                        f"reason={_redact_text(str(failure.get('reason', '?')))}"
                    )
    if report.fidelity + 1e-12 < threshold:
        print(
            "extract-quality: threshold failure "
            f"{report.metric_label}={report.fidelity:.4f} min_fidelity={threshold:.4f}",
            file=sys.stderr,
        )


def _print_json_summary(report: SuiteReport) -> None:
    summary: dict[str, Any] = {
        "suite": report.name,
        "evidence": report.evidence,
        "metric": report.metric_label,
        "cases": len(report.results),
        "fidelity": report.fidelity,
        "gold_hit": report.gold_hit,
        "gold_total": report.gold_total,
        "structural_candidates": report.structural_candidates,
        "grounding_fidelity": report.grounding_fidelity,
        "failed": report.failed,
        "causes": report.causes,
    }
    summary[report.metric_label] = report.fidelity
    print(json.dumps(summary, sort_keys=True))


def _run(argv: Sequence[str]) -> int:
    parser = _parser()
    try:
        args = parser.parse_args(list(argv))
    except SystemExit as exc:
        return EXIT_USAGE if exc.code is None else int(exc.code)

    if args.self_test:
        return _self_test()
    if args.parity is not None:
        if (
            args.unit_only
            or args.live_only
            or args.historical
            or args.endpoint
            or args.fixtures
            or args.heldout
            or args.fixture_set != "tuning"
            or args.raw_output is not None
        ):
            print(
                "extract-quality: error: --parity is standalone and cannot make fixture or endpoint requests",
                file=sys.stderr,
            )
            return EXIT_USAGE
        try:
            return _print_parity(args.parity)
        except HarnessError as exc:
            print(f"extract-quality: error cause={exc.cause}", file=sys.stderr)
            return EXIT_FIXTURE_ERROR
    if args.unit_only and args.live_only:
        print("extract-quality: error: --unit-only and --live-only are mutually exclusive", file=sys.stderr)
        return EXIT_USAGE
    if args.historical and args.live_only:
        print("extract-quality: error: --historical is a unit-fixture view and cannot be combined with live mode", file=sys.stderr)
        return EXIT_USAGE
    if args.heldout:
        if args.fixture_set not in {"tuning", "heldout"}:
            print("extract-quality: error: --heldout cannot be combined with --set both", file=sys.stderr)
            return EXIT_USAGE
        args.fixture_set = "heldout"
    if args.unit_only and args.endpoint:
        print("extract-quality: live suppressed by --unit-only; no endpoint request was attempted")
    if not math.isfinite(args.temperature):
        print("extract-quality: error: --temperature must be finite", file=sys.stderr)
        return EXIT_USAGE
    try:
        fixture_sets = _load_selected_sets(args.fixture_set, args.fixtures)
        if args.live_only and not args.endpoint:
            available = sum(len(fixture_set.cases) for fixture_set in fixture_sets)
            print(
                "extract-quality: live unavailable; attempted_cases=0 "
                f"fixture_cases={available} extracted=0 structural_candidates=0 "
                "admitted=0 grounded=0 failed=0 live_fidelity=0.0000 "
                "causes=endpoint_unavailable"
            )
            print("extract-quality: --live-only requires an explicit --endpoint", file=sys.stderr)
            return EXIT_USAGE
        reports: list[SuiteReport] = []
        if not args.live_only:
            unit = run_unit(fixture_sets, historical=args.historical)
            reports.append(unit)
            _print_report(unit, args.min_fidelity)
        if not args.unit_only:
            if args.endpoint:
                live = run_live(
                    fixture_sets,
                    args.endpoint,
                    args.model,
                    args.temperature,
                    args.timeout,
                    args.request_shape,
                    args.raw_output,
                    no_retry=args.no_retry,
                )
                reports.append(live)
                _print_report(live, args.min_fidelity)
                if live.raw_records:
                    destination = "redacted file and memory" if args.raw_output else "memory"
                    print(
                        "extract-quality: live raw responses captured safely in "
                        f"{destination}; records={len(live.raw_records)}"
                    )
            else:
                print("extract-quality: live skipped; endpoint not supplied; no live result was fabricated")
        if not reports:
            raise HarnessError("fixture_selection", "no quality suite selected")
        total = _aggregate(reports)
        _print_report(total, args.min_fidelity)
        if args.json:
            _print_json_summary(total)
        threshold_failed = total.fidelity + 1e-12 < args.min_fidelity
        if threshold_failed or total.failed:
            print(
                "extract-quality: FAIL "
                f"{total.metric_label}={total.fidelity:.4f} min_fidelity={args.min_fidelity:.4f} "
                f"failed_cases={total.failed} causes={format_causes(total.causes)}",
                file=sys.stderr,
            )
            return EXIT_QUALITY_FAIL
        print(
            f"extract-quality: PASS {total.metric_label}={total.fidelity:.4f} "
            f"min_fidelity={args.min_fidelity:.4f}"
        )
        return EXIT_OK
    except HarnessError as exc:
        print(f"extract-quality: error cause={exc.cause}", file=sys.stderr)
        return EXIT_FIXTURE_ERROR


def main(argv: Sequence[str] | None = None) -> int:
    return _run(sys.argv[1:] if argv is None else argv)


if __name__ == "__main__":
    sys.exit(main())
