#!/usr/bin/env python3
"""Tests for the MCP budget decision record and the row it cites.

The record is validated as data against the committed presets record, so every
rule is refused without the tokenizer: a value the cited row does not state, a
record that names its own landing commit, and a policy that would let a decision
gate a check are each refused with one named ``decisions:`` line. The committed
record is also read back field by field, so the one chosen value on file cannot
drift from the measurement that supports it, and its ``base_commit`` is resolved
with git, so a stale or unreachable commit is caught; that test asserts hard and
skips only when the commit object is absent from a shallow checkout.
"""

import contextlib
import copy
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import budget_decisions

DECISIONS = HERE / "decisions" / "frozen-v1.json"
PRESETS = HERE / "presets" / "frozen-v1.json"
REPO_ROOT = HERE.parents[1]
GIT = os.environ.get("GIT", "git")
PACK_PATH = "benchmarks/mcp_token_budget/presets/packs/{preset}.json"

DERIVED_FROM = {
    "record": "benchmarks/mcp_token_budget/presets/frozen-v1.json",
    "record_id": "preset-surface-v1",
    "row": "minimal",
    "catalog_hash": "fnv1a64:bd68f323db04ccb3",
    "input_sha256": "ee86e2bae172ce2dc3d9ce0ffd4cc5094af05d2e3c95e5a2dc362984896ba67a",
}


def document():
    """Return the committed decision record as a mutable document."""
    return json.loads(DECISIONS.read_text(encoding="utf-8"))


def presets():
    """Return the committed presets record as a mutable document."""
    return json.loads(PRESETS.read_text(encoding="utf-8"))


def git(*args):
    """Run ``git`` in the repository root and return the completed process."""
    return subprocess.run(
        [GIT, *args],
        cwd=str(REPO_ROOT),
        check=False,
        capture_output=True,
        text=True,
    )


def record(value):
    """Return the one decision record in ``value``."""
    return value["records"][0]


def row(value, preset="minimal"):
    """Return the presets row for ``preset``."""
    return next(item for item in value["rows"] if item["preset"] == preset)


class CommittedRecordTests(unittest.TestCase):
    """The committed record states one chosen value and the row that supports it."""

    def test_the_committed_record_holds_one_decision(self):
        records = budget_decisions.load(DECISIONS)
        self.assertEqual(len(records), 1)
        decision = records[0]
        self.assertEqual(list(decision), list(budget_decisions.RECORD_KEYS))
        self.assertEqual(decision["decision_id"], "mcp-minimal-preset-size-2026-11-09")
        self.assertEqual(decision["date"], "2026-11-09")
        self.assertEqual(decision["label"], "Decision")
        self.assertEqual(decision["judgement"], "derived")
        self.assertEqual(decision["preset"], "minimal")
        self.assertEqual(decision["bytes"], 13232)
        self.assertEqual(decision["tokens"], 2803)
        self.assertEqual(decision["derived_from"], DERIVED_FROM)
        self.assertEqual(
            decision["base_commit"], "1720cbd69a5e8bed6be2757fdc20e394018fa777"
        )
        self.assertIn("13232 bytes and 2803 tokens", decision["reason"])
        self.assertTrue(
            decision["reason"].startswith(
                "The minimal preset is the six tools in the retrieval and graph groups"
            )
        )
        self.assertIn("the default turn never calls", decision["reason"])

    def test_the_committed_record_is_never_a_gate_input(self):
        value = document()
        self.assertIs(value["policy"]["gate_input"], False)
        self.assertIn("never a gate input", value["policy"]["rule"])
        self.assertEqual(value["policy"]["rule"].count("never a measurement"), 1)

    def test_the_committed_record_names_no_landing_commit(self):
        value = document()
        for key in budget_decisions.COMMIT_KEYS:
            self.assertNotIn(key, record(value))
            self.assertNotIn(key, record(value)["derived_from"])

    def test_the_cited_row_supports_the_chosen_value(self):
        decision = budget_decisions.load(DECISIONS)[0]
        committed = presets()
        cited = row(committed, decision["preset"])
        self.assertEqual(cited["label"], "Measured")
        self.assertEqual(cited["bytes"], decision["bytes"])
        self.assertEqual(cited["tokens"], decision["tokens"])
        self.assertEqual(cited["catalog_hash"], decision["derived_from"]["catalog_hash"])
        self.assertEqual(cited["input_sha256"], decision["derived_from"]["input_sha256"])
        self.assertEqual(cited["date"], decision["date"])
        self.assertEqual(decision["derived_from"]["record"], budget_decisions.PRESETS_PATH)
        self.assertEqual(decision["derived_from"]["row"], decision["preset"])
        self.assertEqual(committed["record_id"], decision["derived_from"]["record_id"])

    def test_the_module_paths_are_the_committed_records(self):
        self.assertEqual(REPO_ROOT / budget_decisions.DECISIONS_PATH, DECISIONS)
        self.assertEqual(REPO_ROOT / budget_decisions.PRESETS_PATH, PRESETS)
        self.assertEqual(
            budget_decisions.load.__defaults__, (budget_decisions.PRESETS_PATH,)
        )


class BaseCommitTests(unittest.TestCase):
    """The measured ``base_commit`` exists, precedes HEAD, and holds the cited row."""

    def test_the_base_commit_is_an_ancestor_that_holds_the_cited_row(self):
        decision = budget_decisions.load(DECISIONS)[0]
        commit = decision["base_commit"]
        present = git("cat-file", "-e", f"{commit}^{{commit}}")
        if present.returncode != 0:
            self.skipTest(
                f"base_commit {commit} is not in this checkout, which CI checks out "
                f"shallow: {present.stderr.strip() or 'git cat-file -e failed'}"
            )
        ancestor = git("merge-base", "--is-ancestor", commit, "HEAD")
        self.assertEqual(
            ancestor.returncode,
            0,
            f"base_commit {commit} is not an ancestor of HEAD: {ancestor.stderr.strip()}",
        )
        shown = git("show", f"{commit}:{budget_decisions.PRESETS_PATH}")
        self.assertEqual(shown.returncode, 0, shown.stderr.strip())
        cited = decision["derived_from"]
        rows = [
            item for item in json.loads(shown.stdout)["rows"]
            if item["preset"] == decision["preset"]
        ]
        self.assertEqual(len(rows), 1, f"{commit} holds {len(rows)} cited rows")
        stated = rows[0]
        for field, expected in (
            ("bytes", decision["bytes"]),
            ("tokens", decision["tokens"]),
            ("input_sha256", cited["input_sha256"]),
        ):
            with self.subTest(field=field):
                self.assertEqual(
                    stated[field],
                    expected,
                    f"{commit} states {field} {stated[field]!r}, the record cites {expected!r}",
                )
        # That commit's presets row predates the row-level catalog_hash, so the
        # hash it records for the same catalog lives in the pack the row pins.
        pack = git("show", f"{commit}:{PACK_PATH.format(preset=decision['preset'])}")
        self.assertEqual(pack.returncode, 0, pack.stderr.strip())
        self.assertEqual(
            json.loads(pack.stdout)["catalog_hash"],
            cited["catalog_hash"],
            f"{commit} states a different catalog_hash for the cited pack",
        )


class RecordTests(unittest.TestCase):
    """Every malformed decision is refused with one named line."""

    def load(self, value, presets_value=None):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "frozen-v1.json"
            path.write_text(json.dumps(value), encoding="utf-8")
            presets_path = PRESETS
            if presets_value is not None:
                presets_path = Path(tmp) / "presets.json"
                presets_path.write_text(json.dumps(presets_value), encoding="utf-8")
            return budget_decisions.load(path, presets_path)

    def assert_refused(self, value, expected, presets_value=None):
        with self.assertRaises(budget_decisions.DecisionError) as caught:
            self.load(value, presets_value)
        message = str(caught.exception)
        self.assertTrue(message.startswith("decisions: "), message)
        self.assertIn(expected, message)
        return message

    def test_a_value_the_cited_row_does_not_state_is_refused(self):
        for key, wrong in (("bytes", 17320), ("tokens", 3651)):
            with self.subTest(key=key):
                value = document()
                record(value)[key] = wrong
                message = self.assert_refused(value, f"{key} is {wrong}")
                self.assertIn("'minimal'", message)
                self.assertIn("states", message)
        for key, wrong in (
            ("catalog_hash", "fnv1a64:0000000000000000"),
            ("input_sha256", "ab" * 32),
        ):
            with self.subTest(key=key):
                value = document()
                record(value)["derived_from"][key] = wrong
                message = self.assert_refused(value, f"{key} is {wrong!r}")
                self.assertIn("'minimal'", message)

    def test_a_cited_row_that_does_not_exist_is_refused_by_name(self):
        value = document()
        record(value)["preset"] = "turbo"
        record(value)["derived_from"]["row"] = "turbo"
        message = self.assert_refused(value, "cites preset 'turbo'")
        self.assertIn(budget_decisions.PRESETS_PATH, message)
        self.assertIn("holds 0 times", message)

    def test_a_cited_row_labelled_something_else_is_refused(self):
        with_presets = presets()
        row(with_presets)["label"] = "Inferred"
        self.assert_refused(
            document(),
            "cited row label is 'Inferred', expected 'Measured'",
            with_presets,
        )

    def test_a_commit_field_is_refused_by_name(self):
        for key in budget_decisions.COMMIT_KEYS:
            with self.subTest(key=key):
                value = document()
                record(value)[key] = "0" * 40
                message = self.assert_refused(value, f"carries a {key!r} field")
                self.assertIn("git history", message)
        nested = document()
        record(nested)["derived_from"]["landing_commit"] = "0" * 40
        self.assert_refused(nested, "carries a 'landing_commit' field")

    def test_a_base_commit_that_is_not_40_hex_is_refused(self):
        for bad in ("0352da7", "0" * 39, "0" * 41, "A" * 40, 1234, None):
            with self.subTest(base_commit=bad):
                value = document()
                record(value)["base_commit"] = bad
                self.assert_refused(value, "base_commit is not 40 lowercase hex")

    def test_a_gate_input_policy_is_refused(self):
        for bad in (True, 0, "false"):
            with self.subTest(gate_input=bad):
                value = document()
                value["policy"]["gate_input"] = bad
                self.assert_refused(value, "policy.gate_input is")
        empty_rule = document()
        empty_rule["policy"]["rule"] = ""
        self.assert_refused(empty_rule, "policy.rule must be a nonempty string")

    def test_a_label_or_judgement_other_than_the_fixed_ones_is_refused(self):
        value = document()
        record(value)["label"] = "Measured"
        self.assert_refused(value, "label is 'Measured', expected 'Decision'")
        value = document()
        record(value)["judgement"] = "measured"
        self.assert_refused(value, "judgement is 'measured', expected 'derived'")

    def test_a_duplicate_decision_id_is_refused(self):
        value = document()
        value["records"].append(copy.deepcopy(record(value)))
        self.assert_refused(
            value, "duplicate decision_id: 'mcp-minimal-preset-size-2026-11-09'"
        )

    def test_records_out_of_date_order_are_refused(self):
        value = document()
        earlier = copy.deepcopy(record(value))
        earlier["decision_id"] = "mcp-earlier-decision-2026-11-05"
        earlier["date"] = "2026-11-05"
        value["records"].append(earlier)
        message = self.assert_refused(value, "records are not ordered by date")
        self.assertIn("2026-11-05", message)
        self.assertIn("2026-11-09", message)

    def test_keys_must_be_exact_at_every_level(self):
        extra = document()
        extra["extra"] = 1
        self.assert_refused(extra, "top-level keys must be exactly")
        missing = document()
        missing.pop("policy")
        self.assert_refused(missing, "top-level keys must be exactly")
        value = document()
        record(value)["extra"] = 1
        self.assert_refused(value, "keys must be exactly")
        value = document()
        record(value).pop("derived_from")
        self.assert_refused(value, "keys must be exactly")
        value = document()
        record(value)["derived_from"]["extra"] = 1
        self.assert_refused(value, "derived_from keys must be exactly")

    def test_the_format_version_and_record_id_are_fixed(self):
        value = document()
        value["format"] = "other"
        self.assert_refused(value, "format is 'other'")
        value = document()
        value["format_version"] = 2
        self.assert_refused(value, "format_version is 2")
        value = document()
        value["record_id"] = "other"
        self.assert_refused(value, "record_id is 'other'")
        value = document()
        value["frozen"] = False
        self.assert_refused(value, "top-level keys must be exactly")

    def test_a_citation_of_another_record_is_refused(self):
        value = document()
        record(value)["derived_from"]["record"] = (
            "benchmarks/mcp_token_budget/targets/frozen-v1.json"
        )
        self.assert_refused(value, "derived_from.record is")
        value = document()
        record(value)["derived_from"]["record_id"] = "budget-targets-v1"
        self.assert_refused(value, "derived_from.record_id is")
        value = document()
        record(value)["derived_from"]["row"] = "coding"
        self.assert_refused(value, "derived_from.row is")

    def test_non_positive_and_boolean_measures_are_refused(self):
        for key in ("bytes", "tokens"):
            for bad in (0, -1, True, 1.5, "17321"):
                with self.subTest(key=key, value=bad):
                    value = document()
                    record(value)[key] = bad
                    self.assert_refused(value, f"{key} is not a positive integer")

    def test_a_date_reason_or_preset_that_is_not_stated_is_refused(self):
        value = document()
        record(value)["date"] = "2026-11-6"
        self.assert_refused(value, "date is not YYYY-MM-DD")
        value = document()
        record(value)["reason"] = "  "
        self.assert_refused(value, "reason must be a nonempty string")
        value = document()
        record(value)["preset"] = ""
        self.assert_refused(value, "preset must be a nonempty string")

    def test_an_unreadable_or_non_json_file_is_refused(self):
        with self.assertRaises(budget_decisions.DecisionError) as caught:
            budget_decisions.load(HERE / "decisions" / "absent.json")
        self.assertIn("decisions: cannot read decision file", str(caught.exception))
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "frozen-v1.json"
            path.write_text("not json", encoding="utf-8")
            with self.assertRaises(budget_decisions.DecisionError) as caught:
                budget_decisions.load(path, PRESETS)
        self.assertIn("decisions: decision file is not valid JSON", str(caught.exception))

    def test_the_committed_record_is_not_mutated_by_validation(self):
        value = document()
        before = copy.deepcopy(value)
        self.load(value)
        self.assertEqual(value, before)


class CheckCommandTests(unittest.TestCase):
    """``--check`` validates the record and prints one summary line; a bare run refuses."""

    def test_check_reports_one_record_and_exits_zero(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = budget_decisions.main(["--check"])
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "decisions: 1 record, ok\n")

    def test_a_refused_record_exits_two_with_one_named_line(self):
        refusal = budget_decisions.DecisionError("decisions: broken record")
        err = io.StringIO()
        with mock.patch.object(budget_decisions, "load", side_effect=refusal):
            with contextlib.redirect_stderr(err):
                code = budget_decisions.main(["--check"])
        self.assertEqual(code, 2)
        self.assertEqual(err.getvalue(), "decisions: broken record\n")

    def test_a_bare_invocation_prints_one_usage_line_and_exits_two(self):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = budget_decisions.main([])
        self.assertEqual(code, 2)
        self.assertEqual(out.getvalue(), "")
        self.assertEqual(err.getvalue(), f"{budget_decisions.USAGE}\n")


if __name__ == "__main__":
    unittest.main()
