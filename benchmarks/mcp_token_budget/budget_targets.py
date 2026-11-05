#!/usr/bin/env python3
"""Load, validate, and compare the frozen MCP preset target record.

A preset target is a ceiling on the same surface every budget row already
measures: the canonical bytes and the cl100k_base tokens of one schema pack. The
record states each ceiling as a factor of the frozen baseline row and carries the
arithmetic that produced it, so a reader recomputes a target instead of trusting
it, and a target that no longer follows its own arithmetic is refused.

Nothing here enforces. ``PRESET_TARGETS_ENFORCED`` is False, so a measured value
above its target is reported as a miss and never changes an exit code; flipping
that one constant is the whole enforcement change.

The record is validated without the tokenizer, so the hermetic job can check it.
Every error is one line prefixed ``targets:`` and names the rule it broke.
"""

import json
import re
from pathlib import Path

FORMAT = "clio_mcp_preset_targets"
FORMAT_VERSION = 1

BASELINE_FORMAT = "clio_mcp_frozen_baseline"
BASELINE_FORMAT_VERSION = 1

#: Digest of the frozen baseline every target was derived from.
PINNED_BASELINE_SHA256 = "d94fbfa8db884d75baf26da5a921bb2b124eb18f3846c670a76b50b9b8e47e82"
BASELINE_PATH = "benchmarks/mcp_token_budget/baseline/frozen-v1.json"

#: The one switch. True makes a miss a failure; False only reports it.
PRESET_TARGETS_ENFORCED = False

PRESET_NAMES = ("minimal", "coding", "ops", "all")
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


class TargetError(ValueError):
    """Raised when a preset target record cannot be trusted."""


def _fail(message):
    raise TargetError(f"targets: {message}")


def _is_int(value) -> bool:
    """Return true for a JSON integer and false for a boolean."""
    return isinstance(value, int) and not isinstance(value, bool)


def _key_message(where, keys, allowed) -> str:
    missing = sorted(allowed - keys)
    unexpected = sorted(keys - allowed)
    parts = []
    if missing:
        parts.append(f"missing {missing}")
    if unexpected:
        parts.append(f"unexpected {unexpected}")
    return f"{where} keys must be exactly {sorted(allowed)}: " + ", ".join(parts)


def _baseline(value) -> dict:
    if not isinstance(value, dict):
        _fail(f"baseline must be an object, got {type(value).__name__}")
    keys = set(value)
    if keys != BASELINE_KEYS:
        _fail(_key_message("baseline", keys, BASELINE_KEYS))
    if value["format"] != BASELINE_FORMAT:
        _fail(f"baseline format is {value['format']!r}, expected {BASELINE_FORMAT!r}")
    if value["format_version"] != BASELINE_FORMAT_VERSION:
        _fail(
            f"baseline format_version is {value['format_version']!r}, "
            f"expected {BASELINE_FORMAT_VERSION!r}"
        )
    digest = value["sha256"]
    if not isinstance(digest, str) or not SHA256_RE.match(digest):
        _fail(f"baseline sha256 is not 64 lowercase hex: {digest!r}")
    if digest != PINNED_BASELINE_SHA256:
        _fail(f"baseline sha256 is {digest}, expected the pinned {PINNED_BASELINE_SHA256}")
    if value["path"] != BASELINE_PATH:
        _fail(f"baseline path is {value['path']!r}, expected {BASELINE_PATH!r}")
    return dict(value)


def _target(item, index) -> dict:
    if not isinstance(item, dict):
        _fail(f"target {index} must be an object, got {type(item).__name__}")
    keys = set(item)
    if keys != TARGET_KEY_SET:
        _fail(_key_message(f"target {index}", keys, TARGET_KEY_SET))
    preset = item["preset"]
    if preset not in PRESET_NAMES:
        _fail(f"target {index} preset is {preset!r}, expected one of {list(PRESET_NAMES)}")
    unit = item["unit"]
    if unit not in UNITS:
        _fail(f"target {preset} unit is {unit!r}, expected one of {list(UNITS)}")
    label = f"target {preset}/{unit}"
    if item["kind"] != KIND:
        _fail(f"{label} kind is {item['kind']!r}, expected {KIND!r}")
    for key in NUMBER_KEYS:
        value = item[key]
        if not _is_int(value) or value <= 0:
            _fail(f"{label} {key} is not a positive integer: {value!r}")
    expected_row = ROW_TEMPLATE.format(unit=unit)
    if item["baseline_row"] != expected_row:
        _fail(
            f"{label} baseline_row is {item['baseline_row']!r}, expected {expected_row!r}"
        )
    expected_target = item["baseline_value"] * item["factor"] // 100
    if item["target"] != expected_target:
        _fail(
            f"{label} target {item['target']} is not floor({item['baseline_value']} * "
            f"{item['factor']} / 100) = {expected_target}"
        )
    expected_headroom = item["baseline_value"] - item["target"]
    if item["headroom"] != expected_headroom:
        _fail(
            f"{label} headroom {item['headroom']} is not baseline_value - target = "
            f"{expected_headroom}"
        )
    return {key: item[key] for key in TARGET_KEYS}


def _coverage(targets) -> None:
    seen = set()
    for target in targets:
        pair = (target["preset"], target["unit"])
        if pair in seen:
            _fail(f"duplicate target for preset {pair[0]!r} and unit {pair[1]!r}")
        seen.add(pair)
    missing = [
        f"{preset}/{unit}" for preset in PRESET_NAMES for unit in UNITS
        if (preset, unit) not in seen
    ]
    if missing:
        _fail("missing targets for " + ", ".join(missing))


def _targets(value) -> list:
    if not isinstance(value, list):
        _fail(f"targets must be a list, got {type(value).__name__}")
    targets = [_target(item, index) for index, item in enumerate(value)]
    _coverage(targets)
    return targets


def validate(document) -> list:
    """Return ``document``'s target entries, or raise on any problem."""
    if not isinstance(document, dict):
        _fail(f"target file must be a JSON object, got {type(document).__name__}")
    keys = set(document)
    if keys != TOP_LEVEL:
        _fail(_key_message("top-level", keys, TOP_LEVEL))
    if document["format"] != FORMAT:
        _fail(f"format is {document['format']!r}, expected {FORMAT!r}")
    if document["format_version"] != FORMAT_VERSION:
        _fail(
            f"format_version is {document['format_version']!r}, "
            f"expected {FORMAT_VERSION!r}"
        )
    _baseline(document["baseline"])
    return _targets(document["targets"])


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


def _measured(candidate, unit) -> int:
    value = candidate.get(unit) if isinstance(candidate, dict) else None
    if not _is_int(value) or value < 0:
        _fail(f"candidate {unit} is not a non-negative integer: {value!r}")
    return value


def compare(targets, baseline_rows, candidate) -> dict:
    """Return the target comparison for one measured candidate.

    ``baseline_rows`` maps a baseline row id to its frozen integer value and
    ``candidate`` holds the measured ``sha256``, ``bytes``, and ``tokens``. A row
    the record names but the baseline does not hold raises instead of comparing
    against an invented zero.
    """
    if not isinstance(baseline_rows, dict):
        _fail(f"baseline rows must be an object, got {type(baseline_rows).__name__}")
    comparisons = []
    failures = []
    for target in targets:
        _frozen_value(baseline_rows, target)
        measured = _measured(candidate, target["unit"])
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
