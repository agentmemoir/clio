#!/usr/bin/env python3
"""Tests for the budget check's arms, allowance records, and baseline loader.

The arm comparisons are driven through `check_pack` with the frozen baseline's own
numbers, so a pass, a byte-only growth, a token-only growth, and every allowance
outcome is proven without touching the tokenizer; the command itself is tested in
`test_budget_command.py`.
"""

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import baseline_provenance
import budget_allowances
import budget_check
from offline_tokenizer import BudgetError

BASELINE = HERE / "baseline" / "frozen-v1.json"
PACK = HERE.parents[1] / "crates" / "clio-mcp" / "tests" / "fixtures" / "schema_pack.json"

PIN = budget_check.PINNED_BASELINE_SHA256
GROWN_DIGEST = "1a" * 32
OTHER_DIGEST = "2b" * 32
BASE_COMMIT = "abcdef0123456789abcdef0123456789abcdef01"
ALLOWANCE_PATH = "benchmarks/mcp_token_budget/baseline/frozen-v1.json"


def frozen_baseline():
    """Return the resolved baseline ceilings and digest of the committed record."""
    return budget_check.baseline_arms(BASELINE)


def frozen_record():
    """Return the committed record those ceilings were read from."""
    return budget_check.load_baseline(BASELINE)[0]


def report_for(baseline, candidate, allowances):
    """Return `check_pack`'s report for the committed record and these inputs."""
    return budget_check.check_pack(baseline, candidate, allowances, frozen_record())


def candidate_from(baseline, byte_growth=0, token_growth=0, digest=GROWN_DIGEST):
    """Return a synthetic candidate that differs from ``baseline`` by the given growth."""
    return {
        "sha256": digest,
        "bytes": baseline["bytes"] + byte_growth,
        "tokens": baseline["tokens"] + token_growth,
    }


def record(**overrides):
    """Return one valid allowance record, changed by ``overrides``."""
    item = {
        "decision_id": "allow-test-1",
        "date": "2026-11-06",
        "reason": "reviewed growth",
        "base_commit": BASE_COMMIT,
        "candidate_sha256": GROWN_DIGEST,
        "max_bytes": 110,
        "max_tokens": 30,
    }
    item.update(overrides)
    return item


def document(records, digest=PIN):
    """Return one metadata-valid allowance document holding ``records``."""
    return {
        "format": budget_allowances.FORMAT,
        "format_version": budget_allowances.FORMAT_VERSION,
        "baseline": {"path": ALLOWANCE_PATH, "sha256": digest},
        "records": records,
    }


def write_json(directory, name, value):
    path = Path(directory) / name
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def arm_by_measure(report, measure):
    return next(arm for arm in report["arms"] if arm["measure"] == measure)


def ceiling_problem(measure, value, ceiling, decision_id="allow-test-1"):
    return (
        f"{measure}: candidate {value} exceeds allowance ceiling {ceiling} "
        f"for decision {decision_id!r}"
    )


class ArmTests(unittest.TestCase):
    """The two independent arms, driven through `check_pack` without the tokenizer."""

    def test_unchanged_pack_passes(self):
        baseline = frozen_baseline()
        report = report_for(
            baseline, candidate_from(baseline), budget_allowances.NONE
        )
        self.assertEqual(report["verdict"], "pass", report)
        self.assertEqual(report["failures"], [])
        self.assertIsNone(report["allowance"])
        for arm in report["arms"]:
            self.assertEqual(arm["verdict"], "pass")
            self.assertEqual(arm["delta"], 0)
            self.assertIsNone(arm["allowance"])
        self.assertEqual([arm["measure"] for arm in report["arms"]], ["bytes", "tokens"])

    def test_bytes_growth_with_unchanged_tokens_names_bytes_only(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=8)
        report = report_for(baseline, candidate, budget_allowances.NONE)
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(arm_by_measure(report, "bytes")["delta"], 8)
        self.assertEqual(arm_by_measure(report, "tokens")["delta"], 0)
        self.assertTrue(
            any(failure.startswith("bytes:") for failure in report["failures"]), report
        )
        self.assertFalse(
            any("tokens" in failure for failure in report["failures"]), report
        )

    def test_tokens_growth_with_unchanged_bytes_names_tokens_only(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, token_growth=6)
        report = report_for(baseline, candidate, budget_allowances.NONE)
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(arm_by_measure(report, "tokens")["delta"], 6)
        self.assertTrue(
            any(failure.startswith("tokens:") for failure in report["failures"]), report
        )
        self.assertFalse(
            any("bytes" in failure for failure in report["failures"]), report
        )

    def test_growth_without_a_record_names_the_digest(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=8)
        report = report_for(baseline, candidate, budget_allowances.skip())
        self.assertIn(GROWN_DIGEST, report["failures"][0])

    def test_growth_within_a_matching_allowance_passes(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=8, token_growth=6)
        records = [record(max_bytes=candidate["bytes"], max_tokens=candidate["tokens"])]
        report = report_for(baseline, candidate, document(records))
        self.assertEqual(report["verdict"], "pass", report)
        self.assertEqual(report["failures"], [])
        self.assertEqual(report["allowance"]["decision_id"], "allow-test-1")
        self.assertEqual(arm_by_measure(report, "bytes")["delta"], 8)
        self.assertEqual(arm_by_measure(report, "tokens")["delta"], 6)
        self.assertEqual(sorted(report["allowance"]), sorted(budget_check.ALLOWANCE_SUMMARY_KEYS))
        self.assertEqual(report_for(baseline, candidate, records)["verdict"], "pass")

    def test_a_bytes_only_breach_fails_the_bytes_arm_only(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=5, token_growth=5)
        ceiling = candidate["bytes"] - 1
        records = [record(max_bytes=ceiling, max_tokens=candidate["tokens"])]
        report = report_for(baseline, candidate, document(records))
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(len(report["failures"]), 1, report)
        self.assertEqual(
            report["failures"], [ceiling_problem("bytes", candidate["bytes"], ceiling)]
        )
        bytes_arm, tokens_arm = (arm_by_measure(report, m) for m in ("bytes", "tokens"))
        self.assertEqual(bytes_arm["verdict"], "fail")
        self.assertEqual(bytes_arm["failure"], report["failures"][0])
        self.assertEqual(tokens_arm["verdict"], "pass")
        self.assertNotIn("failure", tokens_arm)
        self.assertEqual(tokens_arm["allowance"]["decision_id"], "allow-test-1")

    def test_a_tokens_only_breach_fails_the_tokens_arm_only(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=5, token_growth=5)
        ceiling = candidate["tokens"] - 1
        records = [record(max_bytes=candidate["bytes"], max_tokens=ceiling)]
        report = report_for(baseline, candidate, document(records))
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(len(report["failures"]), 1, report)
        self.assertEqual(
            report["failures"], [ceiling_problem("tokens", candidate["tokens"], ceiling)]
        )
        bytes_arm, tokens_arm = (arm_by_measure(report, m) for m in ("bytes", "tokens"))
        self.assertEqual(tokens_arm["verdict"], "fail")
        self.assertEqual(tokens_arm["failure"], report["failures"][0])
        self.assertEqual(bytes_arm["verdict"], "pass")
        self.assertNotIn("failure", bytes_arm)
        self.assertEqual(bytes_arm["allowance"]["decision_id"], "allow-test-1")

    def test_both_breaches_fail_both_arms_with_two_distinct_messages(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=5, token_growth=5)
        records = [record(max_bytes=candidate["bytes"] - 1,
                          max_tokens=candidate["tokens"] - 1)]
        report = report_for(baseline, candidate, document(records))
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(
            report["failures"],
            [
                ceiling_problem("bytes", candidate["bytes"], candidate["bytes"] - 1),
                ceiling_problem("tokens", candidate["tokens"], candidate["tokens"] - 1),
            ],
        )
        self.assertEqual(len(set(report["failures"])), 2, report)
        self.assertEqual([arm["verdict"] for arm in report["arms"]], ["fail", "fail"])
        self.assertEqual([arm["failure"] for arm in report["arms"]], report["failures"])
        self.assertIsNone(report["allowance"], report)

    def test_exactly_at_both_ceilings_passes_both_arms(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=5, token_growth=5)
        records = [record(max_bytes=candidate["bytes"], max_tokens=candidate["tokens"])]
        report = report_for(baseline, candidate, document(records))
        self.assertEqual(report["verdict"], "pass", report)
        self.assertEqual(report["failures"], [])
        for arm in report["arms"]:
            self.assertEqual(arm["verdict"], "pass")
            self.assertEqual(arm["allowance"]["decision_id"], "allow-test-1")
        self.assertEqual(report["allowance"]["decision_id"], "allow-test-1")

    def test_a_different_digest_with_generous_ceilings_fails(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=8, token_growth=6)
        records = [
            record(candidate_sha256=OTHER_DIGEST, max_bytes=10**6, max_tokens=10**6)
        ]
        report = report_for(baseline, candidate, document(records))
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(len(report["failures"]), 2, report)
        self.assertTrue(
            all(GROWN_DIGEST in failure for failure in report["failures"]), report
        )

    def test_an_allowance_for_another_baseline_is_refused(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=8)
        with self.assertRaises(BudgetError) as caught:
            report_for(
                baseline, candidate, document([record()], digest=OTHER_DIGEST)
            )
        self.assertIn("targets a different baseline", str(caught.exception))
        self.assertIn(OTHER_DIGEST, str(caught.exception))

    def test_rendered_report_shows_every_arm_field(self):
        baseline = frozen_baseline()
        candidate = candidate_from(baseline, byte_growth=8)
        rendered = budget_check.render(
            report_for(baseline, candidate, budget_allowances.NONE)
        )
        self.assertTrue(rendered.endswith("}\n"))
        report = json.loads(rendered)
        self.assertEqual(
            list(report),
            ["format", "format_version", "baseline", "candidate", "allowance",
             "arms", "skill", "activated", "targets", "failures", "verdict"],
        )
        self.assertEqual(report["skill"]["status"], "not-measured")
        self.assertEqual(report["activated"]["status"], "not-measured")
        self.assertEqual(report["activated"]["row"], "activated_skill_body/tokens")
        self.assertEqual(report["activated"]["ceiling"], 594)
        self.assertEqual(report["activated"]["failures"], [])
        self.assertIsNone(report["targets"])
        self.assertEqual(report["format"], "clio_mcp_budget_check")
        self.assertEqual(report["format_version"], 1)
        self.assertEqual(report["baseline"], {"path": ALLOWANCE_PATH, "sha256": PIN})
        self.assertEqual(report["candidate"], candidate)
        self.assertEqual(report["verdict"], "fail")
        for arm in report["arms"]:
            for key in ("baseline", "candidate", "allowance", "delta"):
                self.assertIn(key, arm)


class AllowanceRecordTests(unittest.TestCase):
    """Every malformed record file is refused with a named error."""

    def validate(self, value):
        return budget_allowances.validate(value)

    def assert_refused(self, value, expected):
        with self.assertRaises(budget_allowances.AllowanceError) as caught:
            self.validate(value)
        message = str(caught.exception)
        self.assertTrue(message.startswith("allowance: "), message)
        self.assertIn(expected, message)
        return message

    def test_a_valid_file_loads_its_records(self):
        loaded = budget_allowances.validate(document([record()]))
        self.assertEqual(len(loaded["records"]), 1)
        self.assertEqual(list(loaded["records"][0]), list(budget_allowances.RECORD_KEYS))
        self.assertEqual(loaded["baseline"]["sha256"], PIN)
        with tempfile.TemporaryDirectory() as tmp:
            path = write_json(tmp, "allowances.json", document([record()]))
            self.assertEqual(budget_allowances.load(path), loaded["records"])

    def test_an_empty_record_list_is_valid(self):
        self.assertEqual(budget_allowances.validate(document([]))["records"], [])
        self.assertEqual(budget_allowances.NONE, [])
        self.assertEqual(budget_allowances.skip(), [])

    def test_a_missing_or_empty_reason_is_refused(self):
        broken = record()
        broken.pop("reason")
        self.assert_refused(document([broken]), "reason")
        self.assert_refused(document([record(reason="   ")]), "reason is empty")

    def test_a_missing_ceiling_says_a_reason_alone_is_not_an_allowance(self):
        broken = record()
        broken.pop("max_tokens")
        message = self.assert_refused(document([broken]), "max_tokens")
        self.assertIn("reason alone is not an allowance", message)

    def test_a_commit_bearing_record_is_malformed(self):
        for key in ("commit", "landing_commit", "containing_commit"):
            with self.subTest(field=key):
                broken = record()
                broken[key] = BASE_COMMIT
                message = self.assert_refused(document([broken]), key)
                self.assertIn("git history", message)

    def test_bad_field_values_are_refused(self):
        for value in ("2026-11-6", "20261106", "2026-11-06T00:00:00", "06-11-2026"):
            with self.subTest(date=value):
                self.assert_refused(document([record(date=value)]), "date")
        for value in (BASE_COMMIT[:39], BASE_COMMIT.upper(), "g" * 40, "", 1234):
            with self.subTest(base_commit=value):
                self.assert_refused(document([record(base_commit=value)]), "base_commit")
        for value in (GROWN_DIGEST[:63], GROWN_DIGEST.upper(), "z" * 64):
            with self.subTest(digest=value):
                self.assert_refused(
                    document([record(candidate_sha256=value)]), "candidate_sha256"
                )

    def test_a_bool_or_non_positive_ceiling_is_refused(self):
        for key in ("max_bytes", "max_tokens"):
            for value in (True, 0, -1, 1.5, "12"):
                with self.subTest(key=key, value=value):
                    self.assert_refused(document([record(**{key: value})]), key)

    def test_duplicate_ids_and_out_of_order_dates_are_refused(self):
        self.assert_refused(
            document([record(), record(date="2026-11-07")]), "duplicate decision_id"
        )
        self.assert_refused(
            document([record(date="2026-11-07"), record(decision_id="allow-test-2")]),
            "not ordered by date",
        )

    def test_top_level_keys_must_be_exact(self):
        extra = document([])
        extra["extra"] = 1
        self.assert_refused(extra, "top-level keys must be exactly")
        missing = document([])
        missing.pop("records")
        self.assert_refused(missing, "top-level keys must be exactly")

    def test_a_bad_baseline_digest_is_refused(self):
        self.assert_refused(document([], digest="abc"), "baseline sha256")

    def test_an_absolute_or_private_baseline_path_is_refused(self):
        paths = ("/absolute/frozen-v1.json", "benchmarks\\private\\x.json",
                 "private/x.json", "../x.json")
        for path in paths:
            with self.subTest(path=path):
                value = document([])
                value["baseline"]["path"] = path
                self.assert_refused(value, "baseline path is not repo-relative")

    def test_the_wrong_format_or_version_is_refused(self):
        value = document([])
        value["format"] = "other"
        self.assert_refused(value, "format is")
        value = document([])
        value["format_version"] = 2
        self.assert_refused(value, "format_version is")

    def test_an_unreadable_file_is_refused(self):
        with self.assertRaises(budget_allowances.AllowanceError) as caught:
            budget_allowances.load(HERE / "allowances" / "absent.json")
        self.assertIn("allowance: cannot read record file", str(caught.exception))

    def test_applicable_resolves_one_record_and_refuses_two(self):
        records = [record()]
        self.assertIsNone(budget_allowances.applicable(records, OTHER_DIGEST))
        self.assertEqual(budget_allowances.applicable(records, GROWN_DIGEST), records[0])
        ambiguous = [record(), record(decision_id="allow-test-2", date="2026-11-07")]
        with self.assertRaises(budget_allowances.AllowanceError) as caught:
            budget_allowances.applicable(ambiguous, GROWN_DIGEST)
        message = str(caught.exception)
        self.assertIn("ambiguous records for one candidate digest", message)
        self.assertIn("allow-test-1", message)
        self.assertIn("allow-test-2", message)

    def test_within_returns_problems_keyed_by_measure(self):
        fitted, problems = budget_allowances.within(record(), 108, 26)
        self.assertTrue(fitted)
        self.assertEqual(problems, {"bytes": [], "tokens": []})
        fitted, problems = budget_allowances.within(record(max_bytes=105), 108, 26)
        self.assertFalse(fitted)
        self.assertEqual(problems["tokens"], [])
        self.assertEqual(problems["bytes"], [ceiling_problem("bytes", 108, 105)])
        fitted, both = budget_allowances.within(record(max_bytes=105, max_tokens=20), 108, 26)
        self.assertFalse(fitted)
        self.assertEqual(both["bytes"], [ceiling_problem("bytes", 108, 105)])
        self.assertEqual(both["tokens"], [ceiling_problem("tokens", 26, 20)])


class CommittedRecordTests(unittest.TestCase):
    """The committed ceiling record agrees with the frozen rows and the fixture."""

    def test_the_committed_record_is_tied_to_the_frozen_rows(self):
        records = budget_allowances.load(HERE / "allowances" / "frozen-v1.json")
        self.assertTrue(records)
        entry = next(
            record
            for record in records
            if record["decision_id"] == "mcp-baseline-ceiling-2026-11-05"
        )
        baseline, _ = budget_check.load_baseline(BASELINE)
        rows = (budget_check.BASELINE_ROW_BYTES, budget_check.BASELINE_ROW_TOKENS)
        self.assertEqual(
            [entry["max_bytes"], entry["max_tokens"]],
            [budget_check.baseline_value(baseline, row) for row in rows],
        )
        tools = json.loads(PACK.read_text(encoding="utf-8"))["tools"]
        digest = hashlib.sha256(baseline_provenance.canonical_tools(tools)).hexdigest()
        covering = [
            record for record in records if record["candidate_sha256"] == digest
        ]
        self.assertEqual(len(covering), 1, records)


if __name__ == "__main__":
    unittest.main()
