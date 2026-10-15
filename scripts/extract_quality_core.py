"""Shared limits, error type, and data contract for the extraction harness."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Mapping

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


TUNING_FIXTURE = SCRIPT_DIR / "extract_fixtures.json"
HELDOUT_FIXTURE = SCRIPT_DIR / "extract_heldout.json"
VERIFIER_PARITY = SCRIPT_DIR / "verifier_parity.json"


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


def format_causes(causes: Mapping[str, int]) -> str:
    if not causes:
        return "none"
    return ",".join(f"{key}={value}" for key, value in sorted(causes.items()))
