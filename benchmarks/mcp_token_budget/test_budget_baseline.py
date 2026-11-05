#!/usr/bin/env python3
"""Tests for the frozen baseline loader: its pinned digest and its named rows."""

import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import budget_check
from offline_tokenizer import BudgetError

BASELINE = HERE / "baseline" / "frozen-v1.json"

PIN = budget_check.PINNED_BASELINE_SHA256


def write_json(directory, name, value):
    path = Path(directory) / name
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


class BaselineTests(unittest.TestCase):
    def test_the_committed_baseline_is_pinned(self):
        record_value, digest = budget_check.load_baseline(BASELINE)
        self.assertEqual(digest, PIN)
        self.assertEqual(
            budget_check.baseline_value(record_value, budget_check.BASELINE_ROW_BYTES),
            105632,
        )
        self.assertEqual(
            budget_check.baseline_value(record_value, budget_check.BASELINE_ROW_TOKENS),
            21691,
        )

    def test_a_missing_row_or_wrong_unit_raises_instead_of_defaulting(self):
        record_value, _ = budget_check.load_baseline(BASELINE)
        for row_id in (budget_check.BASELINE_ROW_BYTES, budget_check.BASELINE_ROW_TOKENS):
            with self.subTest(row=row_id):
                broken = copy.deepcopy(record_value)
                broken["rows"] = [row for row in broken["rows"] if row["row"] != row_id]
                with self.assertRaises(BudgetError) as caught:
                    budget_check.baseline_value(broken, row_id)
                self.assertIn("budget: missing baseline row", str(caught.exception))
                self.assertIn(row_id, str(caught.exception))
        token_row = {"row": budget_check.BASELINE_ROW_BYTES, "value": 1, "unit": "tokens"}
        with self.assertRaises(BudgetError) as caught:
            budget_check.baseline_value({"rows": [token_row]}, budget_check.BASELINE_ROW_BYTES)
        self.assertIn("unit is", str(caught.exception))

    def test_a_changed_or_absent_baseline_file_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            record_value, _ = budget_check.load_baseline(BASELINE)
            broken = copy.deepcopy(record_value)
            broken["rows"][0]["value"] = 1
            path = write_json(tmp, "frozen-v1.json", broken)
            with self.assertRaises(BudgetError) as caught:
                budget_check.load_baseline(path)
        message = str(caught.exception)
        self.assertTrue(message.startswith("budget: baseline digest is "), message)
        self.assertIn(PIN, message)
        with self.assertRaises(BudgetError) as caught:
            budget_check.load_baseline(HERE / "baseline" / "absent.json")
        self.assertIn("budget: cannot read baseline", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
