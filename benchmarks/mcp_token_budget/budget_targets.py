#!/usr/bin/env python3
"""Load, validate, and compare the frozen MCP preset target record.

A preset target is a ceiling on the same surface every budget row already
measures: the canonical bytes and the cl100k_base tokens of one schema pack. The
record states each ceiling as a factor of the frozen baseline row and carries the
arithmetic that produced it, so a reader recomputes a target instead of trusting
it, and a target that no longer follows its own arithmetic is refused.

Every preset carries a target for both units. The record holds one row per unit
for each preset in ``TARGET_PRESETS``, and a row for any other preset is refused.
A preset that cannot reach the shared-row derived ceiling carries an explicit
recorded adjustment instead: the record preserves its initial row, factor, and
target, names the measured candidate, and fixes a final ceiling that is never
recomputed by a later run.

Nothing here enforces. ``PRESET_TARGETS_ENFORCED`` is False, so a measured value
above its target is reported as a miss and never changes an exit code; flipping
that one constant is the whole enforcement change. An adjustment is not an
exemption: an adjusted preset is still compared with both final ceilings.

The record is validated without the tokenizer, so the hermetic job can check it.
Every error is one line prefixed ``targets:`` and names the rule it broke.
"""

import json
import re
from pathlib import Path

import target_adjustment
from target_fields import (
    TargetError,
    _expect,
    _fail,
    _is_int,
    _key_message,
    _nonempty,
    _object,
    _repo_relative,
)

FORMAT = "clio_mcp_preset_targets"
FORMAT_VERSION = 1

BASELINE_FORMAT = "clio_mcp_frozen_baseline"
BASELINE_FORMAT_VERSION = 1

#: Digest of the frozen baseline every target was derived from.
PINNED_BASELINE_SHA256 = "484b46ac9ee2703920ee7f21f7f1eb16a00feca36336269489bf10e718e3be40"
BASELINE_PATH = "benchmarks/mcp_token_budget/baseline/frozen-v1.json"

#: The tokenizer every token target is counted with, namable without importing it.
TOKENIZER_KEYS = frozenset(
    {"encoding", "implementation", "version", "asset_sha256", "applies_to"}
)
TOKENIZER_ENCODING = "cl100k_base"
TOKENIZER_IMPLEMENTATION = "tiktoken"
TOKENIZER_VERSION = "0.14.0"
TOKENIZER_ASSET_SHA256 = (
    "223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7"
)

POLICY_KEYS = frozenset({"enforced_by", "missing_baseline_row", "foreign_tokenizer"})
POLICY_DECISION = "reject"

BASIS_KEYS = frozenset({"shared_row", "per_preset_measurement", "reason"})

FACTOR_DECISION_KEYS = frozenset(
    {
        "label",
        "date",
        "token_factor",
        "byte_factor",
        "arithmetic",
        "headroom_formula",
    }
)
FACTOR_KEYS = frozenset({"numerator", "denominator"})
#: The decision divides by a hundred, so a target states the numerator as its factor.
FACTOR_DENOMINATOR = 100
DECISION_LABEL = "Decision"
DECISION_DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
ARITHMETIC = "floor(baseline_value * numerator / denominator)"
HEADROOM_FORMULA = "baseline_value - target"
#: The decision block that supplies each unit's factor.
UNIT_FACTORS = {"tokens": "token_factor", "bytes": "byte_factor"}
TARGET_LABEL = "Inferred"

#: The one switch. True makes a miss a failure; False only reports it.
PRESET_TARGETS_ENFORCED = False

#: Every preset carries both targets; ``all`` carries a recorded final ceiling.
TARGET_PRESETS = ("minimal", "coding", "ops", "all")
UNITS = ("tokens", "bytes")

TOP_LEVEL = frozenset(
    {
        "format",
        "format_version",
        "baseline",
        "basis",
        "tokenizer",
        "factor_decision",
        "policy",
        "targets",
        "target_adjustment",
    }
)
BASELINE_KEYS = frozenset({"path", "format", "format_version", "sha256"})
TARGET_KEYS = (
    "preset",
    "kind",
    "unit",
    "baseline_row",
    "baseline_value",
    "factor",
    "target",
    "headroom",
    "label",
)
TARGET_KEY_SET = frozenset(TARGET_KEYS)
NUMBER_KEYS = ("baseline_value", "factor", "target", "headroom")

KIND = "ceiling"
ROW_TEMPLATE = "client/synthetic-eager-client/turn/1/{unit}"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def _baseline(value) -> dict:
    value = _object(value, "baseline", BASELINE_KEYS)
    _expect(value["format"], "baseline format", BASELINE_FORMAT)
    _expect(value["format_version"], "baseline format_version", BASELINE_FORMAT_VERSION)
    _repo_relative(value["path"], "baseline.path")
    digest = value["sha256"]
    if not isinstance(digest, str) or not SHA256_RE.match(digest):
        _fail(f"baseline sha256 is not 64 lowercase hex: {digest!r}")
    if digest != PINNED_BASELINE_SHA256:
        _fail(f"baseline sha256 is {digest}, expected the pinned {PINNED_BASELINE_SHA256}")
    if value["path"] != BASELINE_PATH:
        _fail(f"baseline path is {value['path']!r}, expected {BASELINE_PATH!r}")
    return dict(value)


def _tokenizer(value) -> dict:
    """Return the tokenizer block, or refuse a foreign or incomplete one."""
    value = _object(value, "tokenizer", TOKENIZER_KEYS)
    _expect(value["encoding"], "tokenizer.encoding", TOKENIZER_ENCODING)
    _expect(
        value["implementation"], "tokenizer.implementation", TOKENIZER_IMPLEMENTATION
    )
    _expect(value["version"], "tokenizer.version", TOKENIZER_VERSION)
    _expect(value["asset_sha256"], "tokenizer.asset_sha256", TOKENIZER_ASSET_SHA256)
    _nonempty(value["applies_to"], "tokenizer.applies_to")
    return dict(value)


def _policy(value) -> dict:
    """Return the policy block, or refuse one that would accept a gap."""
    value = _object(value, "policy", POLICY_KEYS)
    _nonempty(value["enforced_by"], "policy.enforced_by")
    _expect(
        value["missing_baseline_row"],
        "policy.missing_baseline_row",
        POLICY_DECISION,
    )
    _expect(value["foreign_tokenizer"], "policy.foreign_tokenizer", POLICY_DECISION)
    return dict(value)


def _basis(value) -> dict:
    """Return the basis block, or refuse one that claims per-preset measurements."""
    value = _object(value, "basis", BASIS_KEYS)
    for key in ("shared_row", "per_preset_measurement"):
        if not isinstance(value[key], bool):
            _fail(f"basis.{key} is {value[key]!r}, expected a boolean")
    _expect(value["shared_row"], "basis.shared_row", True)
    _expect(value["per_preset_measurement"], "basis.per_preset_measurement", False)
    _nonempty(value["reason"], "basis.reason")
    return dict(value)


def _factor(value, name) -> int:
    """Return the numerator of ``name``'s factor, or refuse the factor."""
    where = f"factor_decision.{name}"
    value = _object(value, where, FACTOR_KEYS)
    for key in ("numerator", "denominator"):
        if not _is_int(value[key]) or value[key] <= 0:
            _fail(f"{where}.{key} is not a positive integer: {value[key]!r}")
    if value["denominator"] != FACTOR_DENOMINATOR:
        _fail(
            f"{where}.denominator is {value['denominator']}, expected "
            f"{FACTOR_DENOMINATOR}: a target states the numerator as its factor"
        )
    return value["numerator"]


def _factor_decision(value) -> dict:
    """Return each unit's frozen factor, or refuse the decision block."""
    value = _object(value, "factor_decision", FACTOR_DECISION_KEYS)
    _expect(value["label"], "factor_decision.label", DECISION_LABEL)
    date = value["date"]
    if not isinstance(date, str) or not DECISION_DATE_RE.match(date):
        _fail(f"factor_decision.date is not YYYY-MM-DD: {date!r}")
    factors = {}
    for unit, name in UNIT_FACTORS.items():
        factors[unit] = _factor(value[name], name)
    _expect(value["arithmetic"], "factor_decision.arithmetic", ARITHMETIC)
    _expect(
        value["headroom_formula"], "factor_decision.headroom_formula", HEADROOM_FORMULA
    )
    return factors


def _target(item, index, factors, adjustments) -> dict:
    if not isinstance(item, dict):
        _fail(f"target {index} must be an object, got {type(item).__name__}")
    keys = set(item)
    if keys != TARGET_KEY_SET:
        _fail(_key_message(f"target {index}", keys, TARGET_KEY_SET))
    preset = item["preset"]
    if preset not in TARGET_PRESETS:
        _fail(
            f"target {index} preset is {preset!r}, expected one of "
            f"{list(TARGET_PRESETS)}"
        )
    unit = item["unit"]
    if unit not in UNITS:
        _fail(f"target {preset} unit is {unit!r}, expected one of {list(UNITS)}")
    label = f"target {preset}/{unit}"
    if item["kind"] != KIND:
        _fail(f"{label} kind is {item['kind']!r}, expected {KIND!r}")
    for key in ("baseline_value", "factor", "target"):
        value = item[key]
        if not _is_int(value) or value <= 0:
            _fail(f"{label} {key} is not a positive integer: {value!r}")
    headroom = item["headroom"]
    if not _is_int(headroom):
        if preset in adjustments:
            _fail(f"{label} headroom is not an integer: {headroom!r}")
        _fail(f"{label} headroom is not a positive integer: {headroom!r}")
    if headroom <= 0 and preset not in adjustments:
        _fail(f"{label} headroom is not a positive integer: {headroom!r}")
    expected_row = ROW_TEMPLATE.format(unit=unit)
    if item["baseline_row"] != expected_row:
        _fail(
            f"{label} baseline_row is {item['baseline_row']!r}, expected {expected_row!r}"
        )
    adjusted = adjustments.get(preset)
    if adjusted is None:
        if item["label"] != TARGET_LABEL:
            _fail(f"{label} label is {item['label']!r}, expected {TARGET_LABEL!r}")
        expected_factor = factors[unit]
        if item["factor"] != expected_factor:
            _fail(
                f"{label} factor is {item['factor']}, expected {expected_factor} from "
                f"factor_decision.{UNIT_FACTORS[unit]}.numerator"
            )
        expected_target = item["baseline_value"] * item["factor"] // 100
        if item["target"] != expected_target:
            _fail(
                f"{label} target {item['target']} is not floor("
                f"{item['baseline_value']} * {item['factor']} / 100) = "
                f"{expected_target}"
            )
    else:
        if item["label"] != DECISION_LABEL:
            _fail(f"{label} label is {item['label']!r}, expected {DECISION_LABEL!r}")
        expected_target = adjusted["final_target"][unit]
        if item["target"] != expected_target:
            _fail(
                f"{label} target is {item['target']}, expected the recorded final "
                f"ceiling {expected_target}"
            )
    expected_headroom = item["baseline_value"] - item["target"]
    if item["headroom"] != expected_headroom:
        _fail(
            f"{label} headroom {item['headroom']} is not baseline_value - target = "
            f"{expected_headroom}"
        )
    if expected_headroom < 0 and adjusted is None:
        _fail(f"{label} headroom {expected_headroom} is negative on an unadjusted row")
    return {key: item[key] for key in TARGET_KEYS}


def _coverage(targets) -> None:
    """Refuse a record that does not hold exactly one row per target preset and unit."""
    seen = set()
    for target in targets:
        pair = (target["preset"], target["unit"])
        if pair in seen:
            _fail(f"duplicate target for preset {pair[0]!r} and unit {pair[1]!r}")
        seen.add(pair)
    missing = [
        f"{preset}/{unit}" for preset in TARGET_PRESETS for unit in UNITS
        if (preset, unit) not in seen
    ]
    if missing:
        _fail("missing targets for " + ", ".join(missing))


def _targets(value, factors, adjustments) -> list:
    if not isinstance(value, list):
        _fail(f"targets must be a list, got {type(value).__name__}")
    targets = [
        _target(item, index, factors, adjustments)
        for index, item in enumerate(value)
    ]
    _coverage(targets)
    return targets


def _adjustments(value, targets) -> dict:
    """Return the validated adjustment map, or refuse a contradictory record."""
    return target_adjustment.validate(value, TARGET_PRESETS, UNITS, targets)


def validate(document) -> list:
    """Return ``document``'s target entries, or raise on any problem."""
    if not isinstance(document, dict):
        _fail(f"target file must be a JSON object, got {type(document).__name__}")
    keys = set(document)
    if keys != TOP_LEVEL:
        _fail(_key_message("top-level", keys, TOP_LEVEL))
    if not isinstance(document["targets"], list):
        _fail(
            f"targets must be a list, got {type(document['targets']).__name__}"
        )
    _expect(document["format"], "format", FORMAT)
    _expect(document["format_version"], "format_version", FORMAT_VERSION)
    _baseline(document["baseline"])
    _basis(document["basis"])
    _tokenizer(document["tokenizer"])
    _policy(document["policy"])
    factors = _factor_decision(document["factor_decision"])
    adjustments = _adjustments(document["target_adjustment"], document["targets"])
    return _targets(document["targets"], factors, adjustments)


def load(path) -> list:
    """Return the validated target entries in the record at ``path``."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        _fail(f"cannot read target file: {exc}")
    try:
        document = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        _fail(f"target file is not valid UTF-8: {exc}")
    except json.JSONDecodeError as exc:
        _fail(f"target file is not valid JSON: {exc}")
    return validate(document)


def _frozen_value(baseline_rows, target) -> int:
    row_id = target["baseline_row"]
    label = f"preset {target['preset']!r} unit {target['unit']!r}"
    value = baseline_rows.get(row_id)
    if value is None:
        _fail(
            f"missing baseline row {row_id!r} for {label}: the policy rejects a "
            "missing row instead of using zero"
        )
    if not _is_int(value) or value <= 0:
        _fail(f"baseline row {row_id!r} value is not a positive integer: {value!r}")
    if value != target["baseline_value"]:
        _fail(
            f"baseline row {row_id!r} is {value} in the frozen record, {label} "
            f"states {target['baseline_value']}"
        )
    return value


def _measured(candidate, unit, preset) -> int:
    value = candidate.get(unit) if isinstance(candidate, dict) else None
    if not _is_int(value) or value < 0:
        _fail(
            f"preset {preset!r} candidate {unit} is not a non-negative integer: "
            f"{value!r}"
        )
    return value


def compare(targets, baseline_rows, candidates) -> dict:
    """Return the target comparison for every preset's measured candidate.

    ``baseline_rows`` maps a baseline row id to its frozen integer value and
    ``candidates`` maps a preset name to that preset's measured ``sha256``,
    ``bytes``, and ``tokens``. Every preset is compared for both units, so a
    missing preset or a missing unit raises instead of being skipped. A row the
    record names but the baseline does not hold raises instead of comparing
    against an invented zero.
    """
    if not isinstance(baseline_rows, dict):
        _fail(f"baseline rows must be an object, got {type(baseline_rows).__name__}")
    if not isinstance(candidates, dict):
        _fail(f"candidates must be an object, got {type(candidates).__name__}")
    comparisons = []
    failures = []
    for target in targets:
        _frozen_value(baseline_rows, target)
        candidate = candidates.get(target["preset"])
        if candidate is None:
            _fail(
                f"no measured candidate for preset {target['preset']!r}: every "
                "preset is compared, so pass --preset-bin PATH to measure the "
                "preset live"
            )
        measured = _measured(candidate, target["unit"], target["preset"])
        headroom = target["target"] - measured
        verdict = "pass" if measured <= target["target"] else "miss"
        comparisons.append(
            {
                "preset": target["preset"],
                "unit": target["unit"],
                "target": target["target"],
                "measured": measured,
                "headroom": headroom,
                "verdict": verdict,
            }
        )
        if verdict == "miss" and PRESET_TARGETS_ENFORCED:
            failures.append(
                f"preset {target['preset']!r} {target['unit']}: measured {measured} "
                f"exceeds target {target['target']} by {-headroom} (headroom {headroom})"
            )
    return {"enforced": PRESET_TARGETS_ENFORCED, "comparisons": comparisons, "failures": failures}
