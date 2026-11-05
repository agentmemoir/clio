#!/usr/bin/env python3
"""Tests for the MCP preset target record and its comparison.

The record is validated as data, so every rule is refused without the tokenizer.
The comparison is driven with the frozen baseline's own row values, so a miss,
its reported headroom, and the unenforced switch are proven without measuring
anything.
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
            [(preset, unit) for preset in budget_targets.PRESET_NAMES
             for unit in budget_targets.UNITS],
        )
        minimal_tokens = entry({"targets": targets}, "minimal", "tokens")
        self.assertEqual(minimal_tokens["target"], 15183)
        self.assertEqual(minimal_tokens["baseline_value"], 21691)
        self.assertEqual(minimal_tokens["headroom"], 6508)
        self.assertEqual(
            minimal_tokens["baseline_row"], "client/synthetic-eager-client/turn/1/tokens"
        )

    def test_the_record_is_not_enforcing_yet(self):
        self.assertFalse(budget_targets.PRESET_TARGETS_ENFORCED)

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
        entry(duplicate, "coding", "tokens")["preset"] = "minimal"
        self.assert_refused(duplicate, "duplicate target for preset 'minimal' and unit 'tokens'")
        missing = document()
        missing["targets"].pop()
        self.assert_refused(missing, "missing targets for")

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
        entry(value, "minimal", "bytes")["target"] += 1
        self.assert_refused(value, "is not floor(")
        value = document()
        entry(value, "minimal", "bytes")["factor"] = 70
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
                    entry(value, "minimal", "tokens")[key] = bad
                    self.assert_refused(value, f"{key} is not a positive integer")

    def test_an_unknown_preset_or_unit_is_refused(self):
        value = document()
        entry(value, "minimal", "tokens")["preset"] = "turbo"
        self.assert_refused(value, "preset is 'turbo'")
        value = document()
        entry(value, "minimal", "tokens")["unit"] = "words"
        self.assert_refused(value, "unit is 'words'")

    def test_an_entry_with_the_wrong_keys_or_shape_is_refused(self):
        value = document()
        value["targets"][0]["extra"] = 1
        self.assert_refused(value, "target 0 keys must be exactly")
        value = document()
        value["targets"][0].pop("label")
        self.assert_refused(value, "target 0 keys must be exactly")
        value = document()
        value["targets"][0] = 1
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

    def test_every_target_is_compared_and_a_miss_is_reported(self):
        result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidate())
        self.assertFalse(result["enforced"])
        self.assertEqual(result["failures"], [])
        self.assertEqual(len(result["comparisons"]), 8)
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
        self.assertTrue(all(item["verdict"] == "miss" for item in result["comparisons"]))

    def test_the_minimal_headroom_matches_the_frozen_rows(self):
        result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidate())
        by_unit = {
            item["unit"]: item
            for item in result["comparisons"]
            if item["preset"] == "minimal"
        }
        self.assertEqual(by_unit["tokens"]["headroom"], 15183 - 21691)
        self.assertEqual(by_unit["tokens"]["headroom"], -6508)
        self.assertEqual(by_unit["bytes"]["headroom"], 84505 - 105632)
        self.assertEqual(by_unit["bytes"]["headroom"], -21127)

    def test_a_measured_value_at_or_under_target_passes(self):
        measured = {"sha256": "ab" * 32, "bytes": 84505, "tokens": 15183}
        result = budget_targets.compare(self.targets(), FROZEN_ROWS, measured)
        self.assertTrue(all(item["verdict"] == "pass" for item in result["comparisons"]))
        self.assertTrue(all(item["headroom"] == 0 for item in result["comparisons"]))
        self.assertEqual(result["failures"], [])

    def test_enforcing_turns_each_miss_into_a_failure(self):
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", True):
            result = budget_targets.compare(self.targets(), FROZEN_ROWS, candidate())
        self.assertTrue(result["enforced"])
        self.assertEqual(len(result["failures"]), 8)
        tokens = next(
            failure
            for failure in result["failures"]
            if "minimal" in failure and "tokens" in failure
        )
        for expected in ("minimal", "tokens", "15183", "21691", "-6508"):
            self.assertIn(expected, tokens)
        self.assertTrue(all(item["verdict"] == "miss" for item in result["comparisons"]))

    def test_a_missing_baseline_row_raises_instead_of_comparing_zero(self):
        rows = dict(FROZEN_ROWS)
        rows.pop("client/synthetic-eager-client/turn/1/tokens")
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.compare(self.targets(), rows, candidate())
        message = str(caught.exception)
        self.assertIn("targets: missing baseline row", message)
        self.assertIn("client/synthetic-eager-client/turn/1/tokens", message)
        self.assertIn("instead of using zero", message)

    def test_a_row_that_disagrees_with_the_frozen_value_raises(self):
        rows = dict(FROZEN_ROWS)
        rows["client/synthetic-eager-client/turn/1/bytes"] = 105631
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.compare(self.targets(), rows, candidate())
        message = str(caught.exception)
        self.assertIn("in the frozen record", message)
        self.assertIn("105631", message)

    def test_a_candidate_without_a_measure_raises(self):
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.compare(self.targets(), FROZEN_ROWS, {"sha256": "ab" * 32})
        self.assertIn("candidate tokens is not a non-negative integer", str(caught.exception))

    def test_the_comparison_does_not_mutate_the_record(self):
        targets = self.targets()
        before = copy.deepcopy(targets)
        budget_targets.compare(targets, FROZEN_ROWS, candidate())
        self.assertEqual(targets, before)


if __name__ == "__main__":
    unittest.main()
