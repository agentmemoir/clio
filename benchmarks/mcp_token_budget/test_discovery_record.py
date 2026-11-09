#!/usr/bin/env python3
"""Tests for the committed discovery cost record.

Every check here reads ``presets/discovery-v1.json`` alone: no server is
started, no release binary is needed, and no network is used. The record must
hold the same-build pair for the ``minimal`` preset, must derive its cost as
on-minus-off, and must keep the frozen pre-flag records apart from that pair.

The date each pre-flag record contributes is derived from its served surface
rows alone, so the last tests below hand the derivation small records of their
own: a later row of another role must not move the date, and a record with no
surface row, an undated surface row, or surface rows that disagree must be
refused instead of answered with a guess.
"""

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import discovery_rows

RECORD = HERE / "presets" / "discovery-v1.json"
BASELINE = HERE / "baseline" / "frozen-v1.json"
PRE_FLAG_PATHS = (
    "benchmarks/mcp_token_budget/presets/frozen-v1.json",
    "benchmarks/mcp_token_budget/baseline/frozen-v1.json",
)
PRE_FLAG_DATES = ("2026-11-06", "2026-11-05")
EAGER_TOKENS = "client/synthetic-eager-client/turn/1/tokens"
EAGER_BYTES = "client/synthetic-eager-client/turn/1/bytes"
ON_DEMAND_BYTES = "client/synthetic-on-demand-client/turn/1/bytes"
ROW_KEYS = {"flag", "label", "tools", "bytes", "tokens", "input_sha256"}
RECORD_KEYS = {
    "format",
    "format_version",
    "record_id",
    "preset",
    "date",
    "delivery",
    "build",
    "measurement",
    "rows",
    "cost",
    "comparability",
}


class DiscoveryRecordTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = json.loads(RECORD.read_text(encoding="utf-8"))
        cls.rows = {row["flag"]: row for row in cls.document["rows"]}

    def test_the_record_holds_exactly_two_minimal_rows(self):
        self.assertEqual(set(self.document), RECORD_KEYS)
        self.assertEqual(len(self.document["rows"]), 2)
        self.assertEqual([row["flag"] for row in self.document["rows"]], ["off", "on"])
        self.assertEqual(self.document["preset"], "minimal")
        self.assertEqual(self.document["delivery"], "served-tools-list")
        for row in self.document["rows"]:
            self.assertEqual(set(row), ROW_KEYS)
            self.assertEqual(row["label"], "Measured")
            self.assertRegex(row["input_sha256"], r"^[0-9a-f]{64}$")
            for key in ("tools", "bytes", "tokens"):
                self.assertIsInstance(row[key], int)
                self.assertGreater(row[key], 0)

    def test_the_off_and_on_rows_state_their_tool_counts(self):
        self.assertEqual(self.rows["off"]["tools"], 6)
        self.assertEqual(self.rows["on"]["tools"], 9)

    def test_the_record_names_one_commit_and_version_for_the_pair(self):
        build = self.document["build"]
        self.assertEqual(set(build), {"commit", "version"})
        self.assertRegex(build["commit"], r"^[0-9a-f]{40}$")
        self.assertTrue(build["version"].strip())
        self.assertEqual(self.document["comparability"]["pair"], ["off", "on"])
        self.assertEqual(self.document["comparability"]["shared_build"], build)

    def test_cost_is_derived_as_on_minus_off(self):
        off = self.rows["off"]
        on = self.rows["on"]
        self.assertEqual(self.document["cost"]["bytes"], on["bytes"] - off["bytes"])
        self.assertEqual(self.document["cost"]["tokens"], on["tokens"] - off["tokens"])

    def test_comparability_names_the_pre_flag_records(self):
        block = self.document["comparability"]
        self.assertEqual(
            [item["path"] for item in block["not_comparable_with"]],
            list(PRE_FLAG_PATHS),
        )
        self.assertEqual(
            [item["date"] for item in block["not_comparable_with"]],
            list(PRE_FLAG_DATES),
        )
        self.assertIn("not comparable", block["note"])

    def test_the_note_rules_out_a_comparison_with_a_pre_flag_record(self):
        block = self.document["comparability"]
        note = block["note"].lower()
        self.assertIn("pre-flag records", note)
        self.assertIn("not comparable", note)
        self.assertEqual(
            [item["path"] for item in block["not_comparable_with"]],
            list(PRE_FLAG_PATHS),
        )


class PreFlagRecordDateTests(unittest.TestCase):
    """The date a pre-flag record contributes comes from its surface rows only."""

    def record(self, rows):
        """Write a record holding these rows and return the path to it."""
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "frozen-v1.json"
        path.write_text(json.dumps({"rows": rows}), encoding="utf-8")
        return path

    def test_the_committed_baseline_dates_itself_from_its_client_rows(self):
        self.assertEqual(discovery_rows._record_date(BASELINE), "2026-11-05")

    def test_a_later_dated_row_of_another_role_leaves_the_date_unchanged(self):
        path = self.record(
            [
                {"row": EAGER_TOKENS, "date": "2026-11-05"},
                {"row": "retired_skill_entry_point/tokens", "date": "2026-11-05"},
                {"row": "activated_skill_body/tokens", "date": "2026-11-08"},
                {"role": "future-role", "date": "2027-01-01"},
                {"role": "future-role", "date": "2026-11-05"},
            ]
        )
        self.assertEqual(discovery_rows._record_date(path), "2026-11-05")

    def test_a_served_surface_row_is_found_under_the_records_own_row_key(self):
        path = self.record(
            [
                {"baseline_row": EAGER_TOKENS, "date": "2026-11-06"},
                {"baseline_row": "activated_skill_body", "date": "2026-11-08"},
            ]
        )
        self.assertEqual(discovery_rows._record_date(path), "2026-11-06")

    def test_surface_rows_that_disagree_raise_and_name_both_dates(self):
        path = self.record(
            [
                {"row": EAGER_BYTES, "date": "2026-11-05"},
                {"row": ON_DEMAND_BYTES, "date": "2026-11-09"},
            ]
        )
        with self.assertRaises(discovery_rows.RecordDateError) as caught:
            discovery_rows._record_date(path)
        message = str(caught.exception)
        self.assertIn("2026-11-05", message)
        self.assertIn("2026-11-09", message)
        self.assertIsInstance(caught.exception, discovery_rows.BudgetError)

    def test_a_record_with_no_surface_row_raises(self):
        path = self.record(
            [
                {"row": "activated_skill_body/tokens", "date": "2026-11-08"},
                {"row": "retired_skill_entry_point/bytes", "date": "2026-11-05"},
            ]
        )
        with self.assertRaises(discovery_rows.RecordDateError) as caught:
            discovery_rows._record_date(path)
        self.assertIn("surface row", str(caught.exception))

    def test_a_surface_row_with_no_date_raises(self):
        path = self.record(
            [
                {"row": EAGER_BYTES, "date": "2026-11-05"},
                {"row": EAGER_TOKENS},
            ]
        )
        with self.assertRaises(discovery_rows.RecordDateError) as caught:
            discovery_rows._record_date(path)
        self.assertIn("undated", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
