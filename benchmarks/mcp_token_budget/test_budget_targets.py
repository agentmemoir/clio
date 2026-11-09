#!/usr/bin/env python3
"""Tests for the MCP preset target record and its comparison.

The record is validated as data, so every rule is refused without the tokenizer.
The comparison is driven with the frozen baseline's own row values, so every
preset's comparison, its reported headroom, and the reporting switch are proven
without measuring anything.
"""

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import budget_targets

TARGETS = HERE / "targets" / "frozen-v1.json"

FROZEN_ROWS = {
    "client/synthetic-eager-client/turn/1/bytes": 105632,
    "client/synthetic-eager-client/turn/1/tokens": 21691,
}


def document():
    """Return the committed target record as a mutable document."""
    return json.loads(TARGETS.read_text(encoding="utf-8"))


def entry(value, preset, unit):
    """Return the one target entry for ``preset`` and ``unit``."""
    return next(
        item
        for item in value["targets"]
        if item["preset"] == preset and item["unit"] == unit
    )


def candidate(**overrides):
    """Return one measured candidate at the frozen baseline values."""
    item = {"sha256": "ab" * 32, "bytes": 105632, "tokens": 21691}
    item.update(overrides)
    return item


def candidates(**overrides):
    """Return one candidate per preset, so every target has a measurement."""
    measured = candidate(**overrides)
    return {preset: dict(measured) for preset in budget_targets.TARGET_PRESETS}


class RecordTests(unittest.TestCase):
    """Every malformed target file is refused with one named line."""

    def load(self, value):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "frozen-v1.json"
            path.write_text(json.dumps(value), encoding="utf-8")
            return budget_targets.load(path)

    def assert_refused(self, value, expected):
        with self.assertRaises(budget_targets.TargetError) as caught:
            self.load(value)
        message = str(caught.exception)
        self.assertTrue(message.startswith("targets: "), message)
        self.assertIn(expected, message)
        return message

    def test_the_committed_record_loads_its_eight_targets(self):
        targets = budget_targets.load(TARGETS)
        self.assertEqual(len(targets), 8)
        self.assertEqual(
            [(item["preset"], item["unit"]) for item in targets],
            [(preset, unit) for preset in budget_targets.TARGET_PRESETS
             for unit in budget_targets.UNITS],
        )
        coding_tokens = entry({"targets": targets}, "coding", "tokens")
        self.assertEqual(coding_tokens["target"], 15183)
        self.assertEqual(coding_tokens["baseline_value"], 21691)
        self.assertEqual(coding_tokens["headroom"], 6508)
        self.assertEqual(
            coding_tokens["baseline_row"], "client/synthetic-eager-client/turn/1/tokens"
        )

    def test_every_preset_has_both_targets(self):
        targets = budget_targets.load(TARGETS)
        pairs = {(item["preset"], item["unit"]) for item in targets}
        self.assertEqual(
            pairs,
            {(preset, unit) for preset in budget_targets.TARGET_PRESETS
             for unit in budget_targets.UNITS},
        )

    def test_the_record_is_enforced(self):
        self.assertTrue(budget_targets.PRESET_TARGETS_ENFORCED)

    def test_the_wrong_format_or_version_is_refused(self):
        value = document()
        value["format"] = "other"
        self.assert_refused(value, "format is")
        value = document()
        value["format_version"] = 2
        self.assert_refused(value, "format_version is")

    def test_top_level_keys_must_be_exact(self):
        extra = document()
        extra["extra"] = 1
        self.assert_refused(extra, "top-level keys must be exactly")
        missing = document()
        missing.pop("policy")
        self.assert_refused(missing, "top-level keys must be exactly")

    def test_a_foreign_or_malformed_baseline_digest_is_refused(self):
        value = document()
        value["baseline"]["sha256"] = "ff" * 32
        message = self.assert_refused(value, "expected the pinned")
        self.assertIn(budget_targets.PINNED_BASELINE_SHA256, message)
        for digest in ("abc", budget_targets.PINNED_BASELINE_SHA256.upper(), 1234):
            with self.subTest(digest=digest):
                value = document()
                value["baseline"]["sha256"] = digest
                self.assert_refused(value, "baseline sha256 is not 64 lowercase hex")

    def test_the_baseline_block_is_checked_against_the_pin(self):
        value = document()
        value["baseline"]["path"] = "benchmarks/mcp_token_budget/other.json"
        self.assert_refused(value, "baseline path is")
        value = document()
        value["baseline"]["extra"] = 1
        self.assert_refused(value, "baseline keys must be exactly")

    def test_a_duplicate_or_missing_preset_unit_pair_is_refused(self):
        duplicate = document()
        entry(duplicate, "ops", "tokens")["preset"] = "coding"
        self.assert_refused(duplicate, "duplicate target for preset 'coding' and unit 'tokens'")
        missing = document()
        missing["targets"].pop(1)
        self.assert_refused(missing, "missing targets for minimal/bytes")

    def test_a_kind_other_than_ceiling_is_refused(self):
        value = document()
        entry(value, "ops", "bytes")["kind"] = "budget"
        self.assert_refused(value, "kind is 'budget', expected 'ceiling'")

    def test_a_baseline_row_that_does_not_follow_the_unit_is_refused(self):
        value = document()
        target = entry(value, "all", "bytes")
        target["baseline_row"] = "client/synthetic-eager-client/turn/1/tokens"
        self.assert_refused(value, "baseline_row is")

    def test_broken_target_arithmetic_is_refused(self):
        value = document()
        entry(value, "coding", "bytes")["target"] += 1
        self.assert_refused(value, "is not floor(")
        value = document()
        entry(value, "coding", "bytes")["factor"] = 70
        self.assert_refused(value, "factor is 70, expected 80")

    def test_broken_headroom_arithmetic_is_refused(self):
        value = document()
        entry(value, "coding", "tokens")["headroom"] -= 1
        self.assert_refused(value, "headroom 6507 is not baseline_value - target = 6508")

    def test_non_positive_and_boolean_numbers_are_refused(self):
        for key in budget_targets.NUMBER_KEYS:
            for bad in (0, -1, True, False, 1.5, "15183"):
                with self.subTest(key=key, value=bad):
                    value = document()
                    entry(value, "coding", "tokens")[key] = bad
                    self.assert_refused(value, f"{key} is not a positive integer")

    def test_an_unknown_preset_or_unit_is_refused(self):
        value = document()
        entry(value, "coding", "tokens")["preset"] = "turbo"
        self.assert_refused(value, "preset is 'turbo'")
        value = document()
        entry(value, "coding", "tokens")["unit"] = "words"
        self.assert_refused(value, "unit is 'words'")

    def test_an_entry_with_the_wrong_keys_or_shape_is_refused(self):
        value = document()
        value["targets"][0]["extra"] = 1
        self.assert_refused(value, "target 0 keys must be exactly")
        value = document()
        value["targets"][0].pop("label")
        self.assert_refused(value, "target 0 keys must be exactly")
        value = document()
        value["targets"][2] = 1
        self.assert_refused(value, "must be an object")
        value = document()
        value["targets"] = {}
        self.assert_refused(value, "targets must be a list")

    def test_an_unreadable_or_non_json_file_is_refused(self):
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.load(HERE / "targets" / "absent.json")
        self.assertIn("targets: cannot read target file", str(caught.exception))
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "frozen-v1.json"
            path.write_text("not json", encoding="utf-8")
            with self.assertRaises(budget_targets.TargetError) as caught:
                budget_targets.load(path)
        self.assertIn("targets: target file is not valid JSON", str(caught.exception))


class CompareTests(unittest.TestCase):
    """The comparison reports every target and fails only when enforcing."""

    def targets(self):
        return budget_targets.load(TARGETS)

    def test_every_preset_is_compared_and_a_miss_is_reported(self):
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", False):
            result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidates())
        self.assertFalse(result["enforced"])
        self.assertEqual(result["failures"], [])
        self.assertEqual(len(result["comparisons"]), 8)
        self.assertEqual(
            {item["preset"] for item in result["comparisons"]},
            {"minimal", "coding", "ops", "all"},
        )
        first = result["comparisons"][0]
        self.assertEqual(
            list(first), ["preset", "unit", "target", "measured", "headroom", "verdict"]
        )
        self.assertEqual(
            first,
            {
                "preset": "minimal",
                "unit": "tokens",
                "target": 15183,
                "measured": 21691,
                "headroom": -6508,
                "verdict": "miss",
            },
        )
        self.assertEqual(
            [item["verdict"] for item in result["comparisons"]],
            ["miss", "miss", "miss", "miss", "miss", "miss", "pass", "pass"],
        )

    def test_the_all_target_uses_the_recorded_final_ceiling(self):
        result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidates())
        adjusted = {
            item["unit"]: item
            for item in result["comparisons"]
            if item["preset"] == "all"
        }
        self.assertEqual(adjusted["tokens"]["target"], 30004)
        self.assertEqual(adjusted["bytes"]["target"], 140559)
        self.assertEqual(adjusted["tokens"]["headroom"], 30004 - 21691)
        self.assertEqual(adjusted["bytes"]["headroom"], 140559 - 105632)

    def test_the_final_ceiling_is_not_recomputed(self):
        targets = self.targets()
        all_tokens = next(
            item for item in targets
            if item["preset"] == "all" and item["unit"] == "tokens"
        )
        self.assertEqual(all_tokens["target"], 30004)
        self.assertNotEqual(all_tokens["target"], 15183)
        self.assertEqual(all_tokens["label"], "Decision")

    def test_the_coding_headroom_matches_the_frozen_rows(self):
        result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidates())
        by_unit = {
            item["unit"]: item
            for item in result["comparisons"]
            if item["preset"] == "coding"
        }
        self.assertEqual(by_unit["tokens"]["headroom"], 15183 - 21691)
        self.assertEqual(by_unit["tokens"]["headroom"], -6508)
        self.assertEqual(by_unit["bytes"]["headroom"], 84505 - 105632)
        self.assertEqual(by_unit["bytes"]["headroom"], -21127)

    def test_a_measured_value_at_or_under_target_passes(self):
        measured = {}
        for preset in budget_targets.TARGET_PRESETS:
            if preset == "all":
                measured[preset] = {
                    "sha256": "ab" * 32, "bytes": 140559, "tokens": 30004
                }
            else:
                measured[preset] = {
                    "sha256": "ab" * 32, "bytes": 84505, "tokens": 15183
                }
        result = budget_targets.compare(self.targets(), FROZEN_ROWS, measured)
        self.assertTrue(all(item["verdict"] == "pass" for item in result["comparisons"]))
        self.assertTrue(all(item["headroom"] == 0 for item in result["comparisons"]))
        self.assertEqual(result["failures"], [])

    def test_before_the_flip_a_miss_prints_and_does_not_fail(self):
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", False):
            result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidates())
        self.assertFalse(result["enforced"])
        self.assertTrue(any(item["verdict"] == "miss" for item in result["comparisons"]))
        self.assertEqual(result["failures"], [])

    def test_enforcing_turns_each_miss_into_a_failure(self):
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", True):
            result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidates())
        self.assertTrue(result["enforced"])
        self.assertEqual(len(result["failures"]), 6)
        tokens = next(
            failure
            for failure in result["failures"]
            if "coding" in failure and "tokens" in failure
        )
        for expected in ("coding", "tokens", "15183", "21691", "-6508"):
            self.assertIn(expected, tokens)
        self.assertFalse(any("'all'" in failure for failure in result["failures"]))

    def test_an_adjusted_preset_still_fails_above_its_final_ceiling(self):
        measured = {
            preset: {"sha256": "ab" * 32, "bytes": 84505, "tokens": 15183}
            for preset in budget_targets.TARGET_PRESETS
        }
        measured["all"] = {"sha256": "ab" * 32, "bytes": 140560, "tokens": 30005}
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", True):
            result = budget_targets.compare(self.targets(), FROZEN_ROWS, measured)
        self.assertEqual(len(result["failures"]), 2)
        for failure in result["failures"]:
            self.assertIn("'all'", failure)
            self.assertIn("30005" if "tokens" in failure else "140560", failure)

    def test_a_missing_preset_measurement_raises(self):
        measured = candidates()
        measured.pop("ops")
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.compare(self.targets(), FROZEN_ROWS, measured)
        self.assertIn("targets: no measured candidate for preset 'ops'", str(caught.exception))

    def test_a_missing_baseline_row_raises_instead_of_comparing_zero(self):
        rows = dict(FROZEN_ROWS)
        rows.pop("client/synthetic-eager-client/turn/1/tokens")
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.compare(self.targets(), rows, candidates())
        message = str(caught.exception)
        self.assertIn("targets: missing baseline row", message)
        self.assertIn("client/synthetic-eager-client/turn/1/tokens", message)
        self.assertIn("instead of using zero", message)

    def test_a_row_that_disagrees_with_the_frozen_value_raises(self):
        rows = dict(FROZEN_ROWS)
        rows["client/synthetic-eager-client/turn/1/bytes"] = 105631
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.compare(self.targets(), rows, candidates())
        message = str(caught.exception)
        self.assertIn("in the frozen record", message)
        self.assertIn("105631", message)

    def test_a_candidate_without_a_measure_raises(self):
        measured = candidates()
        measured["coding"] = {"sha256": "ab" * 32}
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.compare(self.targets(), FROZEN_ROWS, measured)
        message = str(caught.exception)
        self.assertIn("preset 'coding' candidate tokens", message)
        self.assertIn("not a non-negative integer", message)

    def test_the_comparison_does_not_mutate_the_record(self):
        targets = self.targets()
        before = copy.deepcopy(targets)
        budget_targets.compare(targets, FROZEN_ROWS, candidates())
        self.assertEqual(targets, before)


if __name__ == "__main__":
    unittest.main()
