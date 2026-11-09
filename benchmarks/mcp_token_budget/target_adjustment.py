#!/usr/bin/env python3
"""Validate the recorded target adjustments in a frozen preset target record.

An adjustment exists for a preset whose measured shipped surface cannot reach
the shared-row derived ceiling without removing shipped tools. It preserves the
initial baseline row, factor, and target, names the measured candidate, and
fixes the final byte and token ceilings. Validation refuses a malformed or
contradictory record, and a later run never recomputes or replaces a final
ceiling.

An adjustment is not an exemption: the preset is still compared with both
ceilings on every run and still fails above its final ceiling. Every error is
one line prefixed ``targets:`` and names the rule it broke.
"""

import re

from target_fields import _fail, _is_int, _nonempty, _object, _repo_relative

ADJUSTMENT_KEYS = frozenset(
    {
        "decision_id",
        "date",
        "preset",
        "initial",
        "final_target",
        "measured_candidate",
        "reason",
    }
)
INITIAL_KEYS = frozenset({"baseline_row", "factor", "target"})
MEASURED_KEYS = frozenset({"path", "sha256", "bytes", "tokens"})
FORBIDDEN_KEY_PARTS = ("exempt", "accept")
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def _units(value, where, units, positive) -> dict:
    """Return one per-unit block, refusing a wrong key set or a bad value."""
    value = _object(value, where, frozenset(units))
    for unit in units:
        item = value[unit]
        if positive:
            if not _is_int(item) or item <= 0:
                _fail(f"{where}.{unit} is not a positive integer: {item!r}")
        else:
            _nonempty(item, f"{where}.{unit}")
    return dict(value)


def _measured(value) -> dict:
    """Return the measured candidate block, or refuse an incomplete one."""
    value = _object(value, "measured_candidate", MEASURED_KEYS)
    _repo_relative(value["path"], "measured_candidate.path")
    digest = value["sha256"]
    if not isinstance(digest, str) or not SHA256_RE.match(digest):
        _fail(f"measured_candidate.sha256 is not 64 lowercase hex: {digest!r}")
    for key in ("bytes", "tokens"):
        if not _is_int(value[key]) or value[key] < 0:
            _fail(
                f"measured_candidate.{key} is not a non-negative integer: "
                f"{value[key]!r}"
            )
    return dict(value)


def _check_rows(preset, initial, final_target, targets, units) -> None:
    """Refuse an initial block that disagrees with the preset's target rows."""
    rows = {}
    for item in targets:
        if isinstance(item, dict):
            rows[(item.get("preset"), item.get("unit"))] = item
    for unit in units:
        row = rows.get((preset, unit))
        if row is None:
            _fail(f"adjustment preset {preset!r} has no target row for unit {unit!r}")
        if row.get("baseline_row") != initial["baseline_row"][unit]:
            _fail(
                f"adjustment preset {preset!r} {unit} baseline_row is "
                f"{initial['baseline_row'][unit]!r}, target row states "
                f"{row.get('baseline_row')!r}"
            )
        if row.get("factor") != initial["factor"][unit]:
            _fail(
                f"adjustment preset {preset!r} {unit} factor is "
                f"{initial['factor'][unit]}, target row states {row.get('factor')!r}"
            )
        baseline_value = row.get("baseline_value")
        if not _is_int(baseline_value) or baseline_value <= 0:
            _fail(
                f"target row for preset {preset!r} unit {unit!r} has no positive "
                f"baseline_value: {baseline_value!r}"
            )
        expected = baseline_value * initial["factor"][unit] // 100
        if initial["target"][unit] != expected:
            _fail(
                f"adjustment preset {preset!r} {unit} initial target "
                f"{initial['target'][unit]} is not floor({baseline_value} * "
                f"{initial['factor'][unit]} / 100) = {expected}"
            )
        if row.get("target") != final_target[unit]:
            _fail(
                f"target row for preset {preset!r} unit {unit!r} is "
                f"{row.get('target')!r}, expected the recorded final ceiling "
                f"{final_target[unit]}"
            )
        if row.get("label") != "Decision":
            _fail(
                f"target row for preset {preset!r} unit {unit!r} label is "
                f"{row.get('label')!r}, expected 'Decision'"
            )


def validate(value, presets, units, targets) -> dict:
    """Return the adjustments keyed by preset, or raise on any problem."""
    if not isinstance(value, list):
        _fail(f"target_adjustment must be a list, got {type(value).__name__}")
    adjustments = {}
    for index, item in enumerate(value):
        where = f"target_adjustment {index}"
        if isinstance(item, dict):
            for key in item:
                if any(part in key for part in FORBIDDEN_KEY_PARTS):
                    _fail(
                        f"{where} carries {key!r}: an adjustment is not an "
                        "exemption and no preset is exempt"
                    )
        item = _object(item, where, ADJUSTMENT_KEYS)
        _nonempty(item["decision_id"], f"{where}.decision_id")
        date = item["date"]
        if not isinstance(date, str) or not DATE_RE.match(date):
            _fail(f"{where}.date is not YYYY-MM-DD: {date!r}")
        preset = item["preset"]
        if preset not in presets:
            _fail(
                f"{where}.preset is {preset!r}, expected one of {list(presets)}"
            )
        if preset in adjustments:
            _fail(f"duplicate target adjustment for preset {preset!r}")
        initial = _object(item["initial"], f"{where}.initial", INITIAL_KEYS)
        baseline_row = _units(
            initial["baseline_row"], f"{where}.initial.baseline_row", units, False
        )
        for unit in units:
            _repo_relative(
                baseline_row[unit], f"{where}.initial.baseline_row.{unit}"
            )
        factor = _units(initial["factor"], f"{where}.initial.factor", units, True)
        target = _units(initial["target"], f"{where}.initial.target", units, True)
        final_target = _units(
            item["final_target"], f"{where}.final_target", units, True
        )
        measured = _measured(item["measured_candidate"])
        _nonempty(item["reason"], f"{where}.reason")
        for unit in units:
            if final_target[unit] != measured[unit]:
                _fail(
                    f"{where}.final_target.{unit} is {final_target[unit]}, "
                    f"expected the measured candidate value {measured[unit]}"
                )
        _check_rows(
            preset,
            {"baseline_row": baseline_row, "factor": factor, "target": target},
            final_target,
            targets,
            units,
        )
        adjustments[preset] = {
            "decision_id": item["decision_id"],
            "date": item["date"],
            "preset": preset,
            "initial": {
                "baseline_row": baseline_row,
                "factor": factor,
                "target": target,
            },
            "final_target": final_target,
            "measured_candidate": measured,
            "reason": item["reason"],
        }
    return adjustments
