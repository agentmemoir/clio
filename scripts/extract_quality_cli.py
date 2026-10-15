"""Command-line surface for the extraction quality harness."""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path
from typing import Any, Sequence

from extract_quality_core import (
    DEFAULT_MIN_FIDELITY,
    DEFAULT_MODEL,
    DEFAULT_TEMPERATURE,
    DEFAULT_TIMEOUT,
    EXIT_FIXTURE_ERROR,
    EXIT_OK,
    EXIT_QUALITY_FAIL,
    EXIT_USAGE,
    HELDOUT_FIXTURE,
    MAX_TIMEOUT,
    TUNING_FIXTURE,
    VERIFIER_PARITY,
    HarnessError,
    SuiteReport,
    format_causes,
)
from extract_quality_evaluate import run_unit
from extract_quality_fixture import load_fixture
from extract_quality_live import run_live
from extract_quality_parity import _print_parity
from extract_quality_redact import _redact_text
from extract_quality_selftest import _self_test

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
