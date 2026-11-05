#!/usr/bin/env python3
"""Contract tests for every block a frozen target record must state exactly.

The record is trusted only if its tokenizer, policy, basis, factor decision and
baseline pointer all say what the frozen rows were measured with, and each
target entry follows the decision it cites. Every test here mutates one field of
the committed record and asserts one named ``targets:`` line that names the
field it refused, so no block can silently drift from the contract.
"""

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


def entry(value, preset, unit):
    """Return the one target entry for ``preset`` and ``unit``."""
    return next(
        item
        for item in value["targets"]
        if item["preset"] == preset and item["unit"] == unit
    )


class ContractTests(unittest.TestCase):
    """A block that disagrees with the frozen contract is refused by name."""

    def assert_refused(self, value, expected):
        with self.assertRaises(budget_targets.TargetError) as caught:
            budget_targets.validate(value)
        message = str(caught.exception)
        self.assertTrue(message.startswith("targets: "), message)
        self.assertIn(expected, message)
        return message

    def test_a_foreign_tokenizer_encoding_is_refused(self):
        value = document()
        value["tokenizer"]["encoding"] = "o200k_base"
        self.assert_refused(value, "tokenizer.encoding is 'o200k_base'")

    def test_a_foreign_tokenizer_implementation_is_refused(self):
        value = document()
        value["tokenizer"]["implementation"] = "other"
        self.assert_refused(value, "tokenizer.implementation is 'other'")

    def test_a_foreign_tokenizer_version_is_refused(self):
        value = document()
        value["tokenizer"]["version"] = "9.9.9"
        self.assert_refused(value, "tokenizer.version is '9.9.9'")

    def test_a_foreign_tokenizer_asset_digest_is_refused(self):
        value = document()
        value["tokenizer"]["asset_sha256"] = "0" * 64
        message = self.assert_refused(value, "tokenizer.asset_sha256 is")
        self.assertIn(budget_targets.TOKENIZER_ASSET_SHA256, message)

    def test_a_blank_tokenizer_applies_to_is_refused(self):
        for blank in ("", "   ", None, 1):
            with self.subTest(blank=blank):
                value = document()
                value["tokenizer"]["applies_to"] = blank
                self.assert_refused(value, "tokenizer.applies_to")

    def test_the_tokenizer_keys_must_be_exact(self):
        value = document()
        value["tokenizer"]["extra"] = 1
        self.assert_refused(value, "tokenizer keys must be exactly")
        value = document()
        value["tokenizer"].pop("applies_to")
        self.assert_refused(value, "tokenizer keys must be exactly")

    def test_accepting_a_missing_baseline_row_is_refused(self):
        value = document()
        value["policy"]["missing_baseline_row"] = "accept"
        self.assert_refused(
            value, "policy.missing_baseline_row is 'accept', expected 'reject'"
        )

    def test_accepting_a_foreign_tokenizer_is_refused(self):
        value = document()
        value["policy"]["foreign_tokenizer"] = "accept"
        self.assert_refused(
            value, "policy.foreign_tokenizer is 'accept', expected 'reject'"
        )

    def test_a_blank_policy_enforced_by_is_refused(self):
        value = document()
        value["policy"]["enforced_by"] = ""
        self.assert_refused(value, "policy.enforced_by")

    def test_the_policy_keys_must_be_exact(self):
        value = document()
        value["policy"]["extra"] = 1
        self.assert_refused(value, "policy keys must be exactly")
        value = document()
        value["policy"].pop("foreign_tokenizer")
        self.assert_refused(value, "policy keys must be exactly")

    def test_claiming_per_preset_measurements_is_refused(self):
        value = document()
        value["basis"]["per_preset_measurement"] = True
        self.assert_refused(
            value, "basis.per_preset_measurement is True, expected False"
        )

    def test_denying_the_shared_row_is_refused(self):
        value = document()
        value["basis"]["shared_row"] = False
        self.assert_refused(value, "basis.shared_row is False, expected True")

    def test_a_blank_basis_reason_is_refused(self):
        for blank in ("", "   ", None):
            with self.subTest(blank=blank):
                value = document()
                value["basis"]["reason"] = blank
                self.assert_refused(value, "basis.reason")

    def test_a_non_boolean_basis_flag_is_refused(self):
        for key, bad in (("shared_row", 1), ("per_preset_measurement", 0)):
            with self.subTest(key=key, bad=bad):
                value = document()
                value["basis"][key] = bad
                self.assert_refused(
                    value, f"basis.{key} is {bad!r}, expected a boolean"
                )
        value = document()
        value["basis"]["reason"] = True
        self.assert_refused(value, "basis.reason")

    def test_the_basis_keys_must_be_exact(self):
        value = document()
        value["basis"]["extra"] = 1
        self.assert_refused(value, "basis keys must be exactly")
        value = document()
        value["basis"].pop("reason")
        self.assert_refused(value, "basis keys must be exactly")

    def test_a_wrong_decision_label_is_refused(self):
        value = document()
        value["factor_decision"]["label"] = "Inferred"
        self.assert_refused(
            value, "factor_decision.label is 'Inferred', expected 'Decision'"
        )

    def test_a_malformed_decision_date_is_refused(self):
        for bad in ("2026-1-5", "05/11/2026", "2026-11-05T00:00:00", 20261105):
            with self.subTest(date=bad):
                value = document()
                value["factor_decision"]["date"] = bad
                self.assert_refused(value, "factor_decision.date is not YYYY-MM-DD")

    def test_a_zero_or_foreign_denominator_is_refused(self):
        for name in ("token_factor", "byte_factor"):
            for bad in (0, -100, True, "100"):
                with self.subTest(name=name, bad=bad):
                    value = document()
                    value["factor_decision"][name]["denominator"] = bad
                    self.assert_refused(
                        value,
                        f"factor_decision.{name}.denominator is not a positive integer",
                    )
        value = document()
        value["factor_decision"]["token_factor"]["denominator"] = 50
        self.assert_refused(value, "factor_decision.token_factor.denominator is 50")

    def test_a_wrong_arithmetic_string_is_refused(self):
        value = document()
        value["factor_decision"]["arithmetic"] = (
            "baseline_value * numerator / denominator"
        )
        self.assert_refused(value, "factor_decision.arithmetic is")

    def test_a_wrong_headroom_formula_is_refused(self):
        value = document()
        value["factor_decision"]["headroom_formula"] = "target - baseline_value"
        self.assert_refused(value, "factor_decision.headroom_formula is")

    def test_a_factor_that_disagrees_with_the_decision_is_refused(self):
        value = document()
        target = entry(value, "minimal", "tokens")
        target["factor"] = 99
        target["target"] = target["baseline_value"] * 99 // 100
        target["headroom"] = target["baseline_value"] - target["target"]
        self.assert_refused(
            value,
            "target minimal/tokens factor is 99, expected 70 from "
            "factor_decision.token_factor.numerator",
        )
        value = document()
        entry(value, "all", "bytes")["factor"] = 99
        self.assert_refused(value, "target all/bytes factor is 99, expected 80")

    def test_the_factor_decision_keys_must_be_exact(self):
        value = document()
        value["factor_decision"]["extra"] = 1
        self.assert_refused(value, "factor_decision keys must be exactly")
        value = document()
        value["factor_decision"].pop("arithmetic")
        self.assert_refused(value, "factor_decision keys must be exactly")

    def test_the_factor_block_keys_must_be_exact(self):
        value = document()
        value["factor_decision"]["token_factor"]["extra"] = 1
        self.assert_refused(
            value, "factor_decision.token_factor keys must be exactly"
        )
        value = document()
        value["factor_decision"]["byte_factor"].pop("denominator")
        self.assert_refused(
            value, "factor_decision.byte_factor keys must be exactly"
        )

    def test_a_baseline_path_that_leaves_the_repo_is_refused(self):
        for bad in (
            "../benchmarks/mcp_token_budget/baseline/frozen-v1.json",
            "benchmarks/../../baseline/frozen-v1.json",
            "benchmarks\\..\\baseline\\frozen-v1.json",
        ):
            with self.subTest(path=bad):
                value = document()
                value["baseline"]["path"] = bad
                self.assert_refused(value, "baseline.path is")
        for bad in (
            "private/baseline/frozen-v1.json",
            "benchmarks/mcp_token_budget/private/frozen-v1.json",
        ):
            with self.subTest(path=bad):
                value = document()
                value["baseline"]["path"] = bad
                self.assert_refused(value, "baseline.path is")
        for bad in (
            "/benchmarks/mcp_token_budget/baseline/frozen-v1.json",
            "C:/benchmarks/mcp_token_budget/baseline/frozen-v1.json",
        ):
            with self.subTest(path=bad):
                value = document()
                value["baseline"]["path"] = bad
                self.assert_refused(value, "baseline.path is")

    def test_the_baseline_keys_must_be_exact(self):
        value = document()
        value["baseline"]["extra"] = 1
        self.assert_refused(value, "baseline keys must be exactly")
        value = document()
        value["baseline"].pop("format_version")
        self.assert_refused(value, "baseline keys must be exactly")

    def test_a_foreign_baseline_format_or_version_is_refused(self):
        value = document()
        value["baseline"]["format"] = "clio_mcp_preset_targets"
        self.assert_refused(value, "baseline format is")
        value = document()
        value["baseline"]["format_version"] = True
        self.assert_refused(value, "baseline format_version is")

    def test_a_label_other_than_inferred_is_refused(self):
        value = document()
        entry(value, "coding", "bytes")["label"] = "Measured"
        self.assert_refused(value, "label is 'Measured', expected 'Inferred'")

    def test_a_target_entry_with_the_wrong_keys_is_refused(self):
        value = document()
        value["targets"][0]["extra"] = 1
        self.assert_refused(value, "target 0 keys must be exactly")
        value = document()
        value["targets"][0].pop("headroom")
        self.assert_refused(value, "target 0 keys must be exactly")

    def test_the_committed_record_still_satisfies_every_block(self):
        self.assertEqual(len(budget_targets.load(TARGETS)), 8)


if __name__ == "__main__":
    unittest.main()
