#!/usr/bin/env python3
"""Tests for the recorded bounded target adjustment of the ``all`` preset.

The adjustment is data in the target record, so every rule is refused without
the tokenizer. The committed record is mutated one field at a time, and each
refusal must name the rule it broke. An adjustment never exempts a preset: the
final ceiling is fixed data and the preset is still compared with it.
"""

import copy
import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import budget_targets

TARGETS = HERE / "targets" / "frozen-v1.json"


def document():
    """Return the committed target record as a mutable document."""
    return json.loads(TARGETS.read_text(encoding="utf-8"))


def adjustment(value):
    """Return the one committed adjustment record."""
    return value["target_adjustment"][0]


class AdjustmentTests(unittest.TestCase):
    """A malformed or contradictory adjustment is refused by name."""

    def assert_refused(self, value, expected):
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.validate(value)
        message = str(caught.exception)
        self.assertTrue(message.startswith("targets: "), message)
        self.assertIn(expected, message)
        return message

    def test_the_committed_adjustment_is_accepted(self):
        targets = budget_targets.load(TARGETS)
        all_rows = [item for item in targets if item["preset"] == "all"]
        self.assertEqual(len(all_rows), 2)
        self.assertEqual(
            {item["unit"]: item["target"] for item in all_rows},
            {"tokens": 30004, "bytes": 140559},
        )
        self.assertEqual({item["label"] for item in all_rows}, {"Decision"})

    def test_final_ceiling_and_row_must_agree(self):
        value = document()
        adjustment(value)["final_target"]["tokens"] = 30003
        self.assert_refused(value, "expected the measured candidate value 30004")
        value = document()
        value["targets"][-2]["target"] = 30003
        self.assert_refused(value, "expected the recorded final ceiling 30004")

    def test_initial_arithmetic_must_be_preserved(self):
        value = document()
        adjustment(value)["initial"]["target"]["tokens"] = 15184
        self.assert_refused(value, "initial target 15184 is not floor(")

    def test_initial_row_and_factor_must_match_the_target_rows(self):
        value = document()
        adjustment(value)["initial"]["factor"]["bytes"] = 70
        self.assert_refused(value, "factor is 70, target row states 80")
        value = document()
        adjustment(value)["initial"]["baseline_row"]["tokens"] = "other/row"
        self.assert_refused(value, "baseline_row is 'other/row'")

    def test_the_measured_candidate_names_a_real_repo_relative_path(self):
        path = adjustment(document())["measured_candidate"]["path"]
        self.assertEqual(path, "crates/clio-mcp/tests/fixtures/schema_pack.json")

    def test_a_measured_candidate_path_that_escapes_is_refused(self):
        for bad, expected in (
            ("/crates/clio-mcp/tests/fixtures/schema_pack.json", "leading slash"),
            ("crates/../../schema_pack.json", "a '..' segment"),
            ("cli:mcp-schema-export:all", "a URI scheme"),
        ):
            with self.subTest(path=bad):
                value = document()
                adjustment(value)["measured_candidate"]["path"] = bad
                self.assert_refused(value, expected)

    def test_measured_candidate_must_match_final_ceiling(self):
        value = document()
        adjustment(value)["measured_candidate"]["tokens"] = 30003
        self.assert_refused(value, "expected the measured candidate value 30003")

    def test_duplicate_preset_is_refused(self):
        value = document()
        value["target_adjustment"].append(copy.deepcopy(adjustment(value)))
        self.assert_refused(value, "duplicate target adjustment for preset 'all'")

    def test_unknown_preset_is_refused(self):
        value = document()
        adjustment(value)["preset"] = "turbo"
        self.assert_refused(value, "preset is 'turbo'")

    def test_a_non_positive_final_ceiling_is_refused(self):
        value = document()
        adjustment(value)["final_target"]["tokens"] = 0
        self.assert_refused(value, "final_target.tokens is not a positive integer")

    def test_a_non_hex_digest_is_refused(self):
        value = document()
        adjustment(value)["measured_candidate"]["sha256"] = "not-hex"
        self.assert_refused(value, "measured_candidate.sha256 is not 64 lowercase hex")

    def test_exemption_field_is_refused(self):
        value = document()
        adjustment(value)["exempt"] = True
        self.assert_refused(value, "an adjustment is not an exemption")

    def test_missing_initial_or_measured_candidate_block_is_refused(self):
        value = document()
        adjustment(value).pop("initial")
        self.assert_refused(value, "target_adjustment 0 keys must be exactly")
        value = document()
        adjustment(value).pop("measured_candidate")
        self.assert_refused(value, "target_adjustment 0 keys must be exactly")

    def test_a_wrong_key_set_is_refused(self):
        value = document()
        adjustment(value)["extra"] = 1
        self.assert_refused(value, "target_adjustment 0 keys must be exactly")
        value = document()
        adjustment(value)["initial"]["extra"] = 1
        self.assert_refused(value, "target_adjustment 0.initial keys must be exactly")


if __name__ == "__main__":
    unittest.main()
