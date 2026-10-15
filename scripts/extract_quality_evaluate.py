"""Unit evaluation of fixtures and the built-in span contract cases."""

from __future__ import annotations

import math
from typing import Any, Iterable, Mapping, Sequence

from extract_quality_candidate import parse_candidate_response
from extract_quality_core import (
    CaseResult,
    FieldSpec,
    FixtureCase,
    FixtureSet,
    HarnessError,
    SuiteReport,
    VerifyConfig,
)
from extract_quality_span import nfc, verify_snapshot

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
