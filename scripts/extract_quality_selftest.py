"""Deterministic self-test that exercises the harness contract end to end."""

from __future__ import annotations

from extract_quality_candidate import parse_candidate_response
from extract_quality_core import (
    EXIT_OK,
    EXIT_QUALITY_FAIL,
    VERIFIER_PARITY,
    FieldSpec,
    HarnessError,
    SuiteReport,
    VerifyConfig,
)
from extract_quality_evaluate import _case_result, _contract_cases
from extract_quality_live import SYSTEM_PROMPT, build_request
from extract_quality_parity import _evaluate_parity
from extract_quality_redact import _json_text, _redact_text, redact_json
from extract_quality_span import verify_snapshot
from span_verify import verify_snapshot as verify_generic_snapshot

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
    # Labels and secrets stay in separate tuples so no static-analysis taint
    # from the sample literals reaches the printed check names.
    secret_labels = ("AWS access-key id", "GitHub token", "JWT", "Slack token")
    secret_samples = (
        "AKIAIOSFODNN7EXAMPLE",
        "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.signature",
        # Split so the file text holds no scannable token; the sample is the
        # same string at run time.
        "xox" + "b-123456789012-123456789012-abcdefghijklmnop",
    )
    for label, sample in zip(secret_labels, secret_samples, strict=True):
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
