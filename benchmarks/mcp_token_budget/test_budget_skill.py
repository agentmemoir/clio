#!/usr/bin/env python3
"""Tests for the always-loaded skill arm and the not-measured report truth.

The skill file is compared with its own frozen rows and never with a pack
allowance, so a growth fails the run even though a record may exist for the
pack. A test that measures tokens needs the pinned tokenizer and skips with a
named reason when tiktoken is absent.
"""

import contextlib
import copy
import hashlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import budget_check
from offline_tokenizer import BudgetError, load

REPO_ROOT = HERE.parents[1]
PACK = REPO_ROOT / "crates" / "clio-mcp" / "tests" / "fixtures" / "schema_pack.json"
BASELINE = HERE / "baseline" / "frozen-v1.json"

BYTES_ROW = "retired_skill_entry_point/bytes"
TOKENS_ROW = "retired_skill_entry_point/tokens"

UNDER = "Wayfinder skill entry point.\n" * 2
BYTE_GROWTH = ("x" * 80 + "\n") * 100
TOKEN_GROWTH = "a " * 1600


def require_tokenizer():
    """Skip with a named reason when tiktoken is absent."""
    try:
        import tiktoken  # noqa: F401
    except ImportError as exc:
        raise unittest.SkipTest(f"tiktoken is not installed: {exc}") from None


def baseline_record():
    """Return the pinned baseline record."""
    return budget_check.load_baseline(BASELINE)[0]


def write_text(directory, name, text):
    path = Path(directory) / name
    path.write_text(text, encoding="utf-8")
    return path


def arm(skill, measure):
    return next(item for item in skill["arms"] if item["measure"] == measure)


class SkillArmTests(unittest.TestCase):
    """The two skill arms against the frozen rows, driven without the tokenizer."""

    def test_the_frozen_rows_are_compared(self):
        skill = budget_check.check_skill(
            baseline_record(), {"sha256": "ab" * 32, "bytes": 6393, "tokens": 1518}
        )
        self.assertEqual(skill["verdict"], "pass", skill)
        self.assertEqual(skill["failures"], [])
        self.assertEqual(arm(skill, "bytes")["baseline"], 6393)
        self.assertEqual(arm(skill, "tokens")["baseline"], 1518)
        self.assertEqual([item["row"] for item in skill["arms"]], [BYTES_ROW, TOKENS_ROW])
        for item in skill["arms"]:
            self.assertIsNone(item["allowance"])
            self.assertEqual(item["delta"], 0)

    def test_byte_growth_names_the_bytes_row_only(self):
        skill = budget_check.check_skill(
            baseline_record(), {"sha256": "ab" * 32, "bytes": 7000, "tokens": 1518}
        )
        self.assertEqual(skill["verdict"], "fail", skill)
        self.assertEqual(len(skill["failures"]), 1, skill)
        self.assertEqual(
            skill["failures"][0],
            f"{BYTES_ROW}: candidate 7000 exceeds baseline 6393 by 607",
        )
        self.assertEqual(arm(skill, "tokens")["verdict"], "pass")

    def test_token_growth_names_the_tokens_row_only(self):
        skill = budget_check.check_skill(
            baseline_record(), {"sha256": "ab" * 32, "bytes": 6393, "tokens": 1600}
        )
        self.assertEqual(skill["verdict"], "fail", skill)
        self.assertEqual(len(skill["failures"]), 1, skill)
        self.assertTrue(skill["failures"][0].startswith(f"{TOKENS_ROW}: "), skill)
        self.assertIn("82", skill["failures"][0])
        self.assertEqual(arm(skill, "bytes")["verdict"], "pass")

    def test_a_missing_row_raises_instead_of_defaulting(self):
        for row_id in (BYTES_ROW, TOKENS_ROW):
            with self.subTest(row=row_id):
                record = copy.deepcopy(baseline_record())
                record["rows"] = [row for row in record["rows"] if row["row"] != row_id]
                with self.assertRaises(BudgetError) as caught:
                    budget_check.check_skill(
                        record, {"sha256": "ab" * 32, "bytes": 1, "tokens": 1}
                    )
                message = str(caught.exception)
                self.assertIn("budget: missing baseline row", message)
                self.assertIn(row_id, message)

    def test_an_allowance_record_never_covers_the_skill_rows(self):
        skill = budget_check.check_skill(
            baseline_record(), {"sha256": "ab" * 32, "bytes": 7000, "tokens": 1518}
        )
        self.assertTrue(skill["failures"])
        for item in skill["arms"]:
            self.assertIsNone(item["allowance"])
        self.assertFalse(any("allowance" in failure for failure in skill["failures"]))


class SkillMeasurementTests(unittest.TestCase):
    """A measured file passes only when both of its arms fit."""

    def measure(self, text):
        require_tokenizer()
        tokenizer = load()
        with tempfile.TemporaryDirectory() as tmp:
            path = write_text(tmp, "wayfinder.md", text)
            measured = budget_check.measure_skill(path, tokenizer)
            expected = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertEqual(measured["sha256"], expected)
        return measured

    def test_a_file_under_both_rows_passes(self):
        measured = self.measure(UNDER)
        self.assertLess(measured["bytes"], 6393)
        self.assertLess(measured["tokens"], 1518)
        skill = budget_check.check_skill(baseline_record(), measured)
        self.assertEqual(skill["verdict"], "pass", skill)
        self.assertEqual(skill["failures"], [])

    def test_growth_in_bytes_only_fails_naming_the_bytes_row(self):
        measured = self.measure(BYTE_GROWTH)
        self.assertGreater(measured["bytes"], 6393)
        self.assertLess(measured["tokens"], 1518)
        skill = budget_check.check_skill(baseline_record(), measured)
        self.assertEqual(skill["verdict"], "fail", skill)
        self.assertEqual(len(skill["failures"]), 1, skill)
        failure = skill["failures"][0]
        self.assertTrue(failure.startswith(f"{BYTES_ROW}: "), failure)
        self.assertIn(str(measured["bytes"]), failure)
        self.assertIn(str(measured["bytes"] - 6393), failure)

    def test_growth_in_tokens_only_fails_naming_the_tokens_row(self):
        measured = self.measure(TOKEN_GROWTH)
        self.assertLess(measured["bytes"], 6393)
        self.assertGreater(measured["tokens"], 1518)
        skill = budget_check.check_skill(baseline_record(), measured)
        self.assertEqual(skill["verdict"], "fail", skill)
        self.assertEqual(len(skill["failures"]), 1, skill)
        failure = skill["failures"][0]
        self.assertTrue(failure.startswith(f"{TOKENS_ROW}: "), failure)
        self.assertIn(str(measured["tokens"]), failure)

    def test_a_non_utf8_file_raises_a_named_error(self):
        require_tokenizer()
        tokenizer = load()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "wayfinder.md"
            path.write_bytes(b"\xff\xfe\x00not utf-8")
            with self.assertRaises(BudgetError) as caught:
                budget_check.measure_skill(path, tokenizer)
        self.assertIn("budget: skill file is not valid UTF-8", str(caught.exception))

    def test_an_absent_skill_file_raises_a_named_error(self):
        require_tokenizer()
        tokenizer = load()
        with self.assertRaises(BudgetError) as caught:
            budget_check.measure_skill(HERE / "absent-skill.md", tokenizer)
        self.assertIn("budget: cannot read skill file", str(caught.exception))


class SkillCommandTests(unittest.TestCase):
    """The command shows the frozen skill rows instead of a zero or a null."""

    def run_main(self, argv):
        out = io.StringIO()
        err = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = budget_check.main(argv)
        return code, out.getvalue(), err.getvalue()

    def test_no_skill_path_reports_not_measured_with_the_frozen_rows(self):
        require_tokenizer()
        code, text, err = self.run_main(["--pack", str(PACK)])
        self.assertEqual(code, 0, text + err)
        report = json.loads(text)
        self.assertNotIn("skill_not_measured", report)
        self.assertEqual(
            report["skill"],
            {
                "status": "not-measured",
                "reason": budget_check.NO_SKILL_PATH,
                "baseline_rows": {
                    "retired_skill_entry_point/bytes": 6393,
                    "retired_skill_entry_point/tokens": 1518,
                },
            },
            report["skill"],
        )
        self.assertEqual(
            list(report["skill"]["baseline_rows"]), [BYTES_ROW, TOKENS_ROW]
        )
        self.assertEqual(report["skill"]["baseline_rows"][BYTES_ROW], 6393)
        self.assertEqual(report["skill"]["baseline_rows"][TOKENS_ROW], 1518)
        self.assertEqual(report["verdict"], "pass")

    def test_an_explicitly_empty_skill_path_exits_two(self):
        code, text, err = self.run_main(["--pack", str(PACK), "--skill", ""])
        self.assertEqual(code, 2, text + err)
        self.assertEqual(text, "")
        self.assertEqual(err, "mcp-budget-check: --skill path is empty\n")

    def test_a_measured_skill_path_reports_its_numbers(self):
        require_tokenizer()
        with tempfile.TemporaryDirectory() as tmp:
            path = write_text(tmp, "wayfinder.md", UNDER)
            expected = hashlib.sha256(path.read_bytes()).hexdigest()
            code, text, err = self.run_main(["--pack", str(PACK), "--skill", str(path)])
        self.assertEqual(code, 0, text + err)
        report = json.loads(text)
        self.assertEqual(report["skill"]["sha256"], expected)
        self.assertEqual(report["skill"]["bytes"], len(UNDER.encode("utf-8")))
        self.assertEqual(report["skill"]["verdict"], "pass")
        self.assertEqual(
            report["skill"]["baseline_rows"],
            {BYTES_ROW: 6393, TOKENS_ROW: 1518},
        )
        self.assertEqual(report["failures"], [])

    def test_skill_growth_fails_the_run_with_exit_one(self):
        require_tokenizer()
        with tempfile.TemporaryDirectory() as tmp:
            path = write_text(tmp, "wayfinder.md", TOKEN_GROWTH)
            code, text, err = self.run_main(["--pack", str(PACK), "--skill", str(path)])
        self.assertEqual(code, 1, text + err)
        report = json.loads(text)
        self.assertEqual(report["verdict"], "fail")
        self.assertEqual(len(report["failures"]), 1, report["failures"])
        self.assertTrue(report["failures"][0].startswith(f"{TOKENS_ROW}: "))
        self.assertEqual(report["arms"][0]["delta"], 0)

    def test_a_non_utf8_skill_path_exits_two(self):
        require_tokenizer()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "wayfinder.md"
            path.write_bytes(b"\xff\xfe\x00not utf-8")
            code, text, err = self.run_main(["--pack", str(PACK), "--skill", str(path)])
        self.assertEqual(code, 2, text + err)
        self.assertEqual(text, "")
        self.assertIn("budget: skill file is not valid UTF-8", err)


if __name__ == "__main__":
    unittest.main()
