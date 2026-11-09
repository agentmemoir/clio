#!/usr/bin/env python3
"""Tests for the always-loaded skill arm and the not-measured report truth.

The skill file is compared with its own frozen rows and never with a pack
allowance, so a growth fails the run even though a record may exist for the
pack. The activated skill body is compared with the frozen row that carries its
token count, by token growth alone. A test that measures tokens needs the
pinned tokenizer and skips with a named reason when tiktoken is absent.
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
from unittest import mock

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import activated_budget
import activated_row
import budget_allowances
import budget_check
import budget_targets
from offline_tokenizer import BudgetError, load

REPO_ROOT = HERE.parents[1]
PACK = REPO_ROOT / "crates" / "clio-mcp" / "tests" / "fixtures" / "schema_pack.json"
BASELINE = HERE / "baseline" / "frozen-v1.json"
TARGETS = HERE / "targets" / "frozen-v1.json"

BYTES_ROW = "retired_skill_entry_point/bytes"
TOKENS_ROW = "retired_skill_entry_point/tokens"
FROZEN_ROWS = {BYTES_ROW: 6393, TOKENS_ROW: 1518}

ACTIVATED_ROW = "activated_skill_body/tokens"
ACTIVATED_PATH = REPO_ROOT / "skills" / "clio-mcp" / "SKILL.md"
ACTIVATED_TOKENS = 594

UNDER = "Wayfinder skill entry point.\n" * 2
BYTE_GROWTH = ("x" * 80 + "\n") * 100
TOKEN_GROWTH = "a " * 1600
OTHER_SHA = "cd" * 32


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


def activated_row_of(record):
    """Return the record's one activated skill body row."""
    return activated_row.rows(record)[0]


def changed_row(**changes):
    """Return a pinned record with the activated row changed by ``changes``."""
    record = baseline_record()
    activated_row_of(record).update(changes)
    return record


def body(tokens, digest=OTHER_SHA, size=1):
    """Return one measured candidate body with the given numbers."""
    return {"sha256": digest, "bytes": size, "tokens": tokens}


def pack_candidate(baseline):
    """Return one measured pack candidate holding ``baseline``'s own numbers."""
    return {"sha256": OTHER_SHA, "bytes": baseline["bytes"], "tokens": baseline["tokens"]}


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
        measured, out, err = {p: {"sha256": "ab" * 32, "bytes": 1, "tokens": 1} for p in budget_targets.TARGET_PRESETS}, io.StringIO(), io.StringIO()
        with mock.patch.object(budget_check.preset_rows, "measure_presets", return_value=measured), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = budget_check.main([*argv, "--preset-bin", "clio"])
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
                "baseline_rows": FROZEN_ROWS,
            },
            report["skill"],
        )
        self.assertEqual(report["verdict"], "pass")
        self.assertEqual(list(report["skill"]["baseline_rows"]), [BYTES_ROW, TOKENS_ROW])

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
        self.assertEqual(report["skill"]["baseline_rows"], FROZEN_ROWS)
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
        self.assertEqual(report["arms"][0]["verdict"], "pass")
        self.assertIsNotNone(report["arms"][0]["allowance"])

    def test_a_non_utf8_skill_path_exits_two(self):
        require_tokenizer()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "wayfinder.md"
            path.write_bytes(b"\xff\xfe\x00not utf-8")
            code, text, err = self.run_main(["--pack", str(PACK), "--skill", str(path)])
        self.assertEqual(code, 2, text + err)
        self.assertEqual(text, "")
        self.assertIn("budget: skill file is not valid UTF-8", err)


class ActivatedBodyTests(unittest.TestCase):
    """The shipped activated body is bounded by token growth and nothing else."""

    def test_the_shipped_body_passes_against_the_frozen_row(self):
        require_tokenizer()
        measured = budget_check.measure_skill(ACTIVATED_PATH, load())
        result = activated_budget.check(baseline_record(), measured)
        self.assertEqual(result["verdict"], "pass", result)
        self.assertEqual(result["failures"], [])
        self.assertEqual(result["row"], ACTIVATED_ROW)
        self.assertEqual(result["tokens"], ACTIVATED_TOKENS)
        self.assertEqual(result["ceiling"], ACTIVATED_TOKENS)
        self.assertEqual(result["delta"], 0)
        self.assertTrue(result["digest_matches"])

    def test_token_growth_names_the_row_and_the_excess(self):
        result = activated_budget.check(baseline_record(), body(ACTIVATED_TOKENS + 106))
        self.assertEqual(result["verdict"], "fail", result)
        self.assertEqual(
            result["failures"],
            [f"{ACTIVATED_ROW}: candidate 700 exceeds baseline 594 by 106"],
        )
        self.assertEqual(result["delta"], 106)

    def test_the_same_tokens_with_other_bytes_and_digest_pass(self):
        result = activated_budget.check(
            baseline_record(), body(ACTIVATED_TOKENS, size=4096)
        )
        self.assertEqual(result["verdict"], "pass", result)
        self.assertEqual(result["failures"], [])
        self.assertFalse(result["digest_matches"])
        self.assertEqual(result["sha256"], OTHER_SHA)
        self.assertEqual(result["frozen_sha256"], activated_row.CONTENT_SHA256)

    def assert_refused(self, record, *named):
        """Assert `check` refuses ``record`` with a message naming each of ``named``."""
        with self.assertRaises(BudgetError) as caught:
            activated_budget.check(record, body(1))
        for needle in named:
            self.assertIn(needle, str(caught.exception))

    def test_a_factor_other_than_one_is_refused(self):
        for factor in (1.5, 0.5, 1, 2, "1.0", True, None):
            with self.subTest(factor=factor):
                self.assert_refused(changed_row(factor=factor), ACTIVATED_ROW, "factor")

    def test_a_missing_row_is_refused(self):
        record = baseline_record()
        record["rows"] = [row for row in record["rows"] if row["row"] != ACTIVATED_ROW]
        self.assert_refused(record, ACTIVATED_ROW, "exactly one")

    def test_a_duplicate_row_is_refused(self):
        record = baseline_record()
        record["rows"].append(copy.deepcopy(activated_row_of(record)))
        self.assert_refused(record, ACTIVATED_ROW, "exactly one")

    def test_a_non_positive_value_is_refused(self):
        for value in (0, -594, True):
            with self.subTest(value=value):
                self.assert_refused(changed_row(value=value), ACTIVATED_ROW, "value")

    def test_a_wrong_unit_is_refused(self):
        self.assert_refused(changed_row(unit="bytes"), ACTIVATED_ROW, "unit")

    def test_a_failing_check_leaves_the_record_untouched(self):
        before = hashlib.sha256(BASELINE.read_bytes()).hexdigest()
        record = baseline_record()
        expected = copy.deepcopy(record)
        grown = activated_budget.check(record, body(10 * ACTIVATED_TOKENS))
        self.assertEqual(grown["verdict"], "fail")
        self.assertEqual(record, expected)
        self.assertEqual(hashlib.sha256(BASELINE.read_bytes()).hexdigest(), before)

    def test_a_larger_candidate_does_not_move_the_ceiling(self):
        record = baseline_record()
        grown = activated_budget.check(record, body(10 * ACTIVATED_TOKENS, size=40000))
        self.assertEqual(grown["verdict"], "fail")
        later = activated_budget.check(record, body(ACTIVATED_TOKENS))
        self.assertEqual(later["ceiling"], ACTIVATED_TOKENS)
        self.assertEqual(later["ceiling"], grown["ceiling"])
        self.assertEqual(later["verdict"], "pass")
        self.assertEqual(activated_budget.frozen_ceiling(record)["ceiling"], ACTIVATED_TOKENS)

    def test_the_retired_rows_do_not_set_the_ceiling(self):
        record = baseline_record()
        for row in record["rows"]:
            if row["row"] != ACTIVATED_ROW:
                row["value"] = 1
        self.assertEqual(activated_budget.frozen_ceiling(record)["ceiling"], ACTIVATED_TOKENS)

    def test_the_targets_do_not_set_the_ceiling(self):
        require_tokenizer()
        record = baseline_record()
        baseline = budget_check.arms_from(record, budget_check.PINNED_BASELINE_SHA256)
        document = json.loads(TARGETS.read_text(encoding="utf-8"))
        document["factor_decision"]["token_factor"]["numerator"] = 1
        for item in document["targets"]:
            if item["unit"] == "tokens" and item["preset"] != "all":
                item["factor"] = 1
                item["target"] = item["baseline_value"] // 100
                item["headroom"] = item["baseline_value"] - item["target"]
        measured = {p: pack_candidate(baseline) for p in budget_targets.TARGET_PRESETS}
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", False):
            targets = budget_targets.compare(
                budget_targets.validate(document), budget_check.baseline_rows(record),
                measured)
        report = budget_check.check_pack(
            baseline, pack_candidate(baseline), budget_allowances.NONE, record,
            targets=targets,
            activated_body=budget_check.measure_skill(ACTIVATED_PATH, load()),
        )
        squeezed = [
            i["target"] for i in targets["comparisons"]
            if i["unit"] == "tokens" and i["preset"] != "all"
        ]
        self.assertLess(max(squeezed), ACTIVATED_TOKENS)
        self.assertEqual(report["activated"]["tokens"], ACTIVATED_TOKENS)
        self.assertEqual(report["activated"]["ceiling"], ACTIVATED_TOKENS)
        self.assertEqual(report["activated"]["row"], ACTIVATED_ROW)
        self.assertEqual(report["activated"]["failures"], [])
        self.assertEqual(report["failures"], [])

    def test_a_lowered_record_row_fails_naming_the_row(self):
        require_tokenizer()
        record = changed_row(value=100)
        baseline = budget_check.arms_from(record, budget_check.PINNED_BASELINE_SHA256)
        report = budget_check.check_pack(
            baseline, pack_candidate(baseline), budget_allowances.NONE, record,
            activated_body=budget_check.measure_skill(ACTIVATED_PATH, load()),
        )
        self.assertEqual(report["activated"]["ceiling"], 100)
        self.assertEqual(report["activated"]["row"], ACTIVATED_ROW)
        self.assertTrue(report["activated"]["digest_matches"])
        self.assertEqual([item["verdict"] for item in report["arms"]], ["pass", "pass"])
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(
            report["failures"],
            [f"{ACTIVATED_ROW}: candidate {ACTIVATED_TOKENS} exceeds baseline 100 "
             f"by {ACTIVATED_TOKENS - 100}"],
        )

    def test_an_activated_failure_reaches_the_pack_report(self):
        record = baseline_record()
        baseline = budget_check.arms_from(record, budget_check.PINNED_BASELINE_SHA256)
        activated = activated_budget.check(record, body(ACTIVATED_TOKENS + 6))
        report = budget_check.check_pack(
            baseline,
            pack_candidate(baseline),
            budget_allowances.NONE,
            record,
            activated=activated,
        )
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(report["failures"], activated["failures"])
        self.assertEqual([item["verdict"] for item in report["arms"]], ["pass", "pass"])


if __name__ == "__main__":
    unittest.main()
