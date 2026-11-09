#!/usr/bin/env python3
"""Tests for the turn statements, ranking tie-break, and unobserved gap.

Each test mutates a copy of the record, copies the fixtures beside it, and
asserts the validator refuses the copy with the expected AC error.
"""

import contextlib
import copy
import hashlib
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import activated_row
import baseline_validate

RECORD = HERE / "baseline" / "frozen-v1.json"
FIXTURES = HERE / "baseline" / "clients"
ACTIVATED_ROW = activated_row.ROW_ID


def load_record():
    return json.loads(RECORD.read_text(encoding="utf-8"))


def activated(record):
    """Return the shipped activated skill body row of ``record``."""
    return next(row for row in record["rows"] if row["row"] == ACTIVATED_ROW)


def refusals(mutate):
    """Return the validator errors for one mutation of the real record."""
    with mutated_record(mutate) as path:
        return baseline_validate.validate(path)


@contextlib.contextmanager
def mutated_record(mutate):
    """Yield a temp record path whose object was changed by ``mutate``."""
    record = copy.deepcopy(load_record())
    mutate(record)
    with tempfile.TemporaryDirectory() as tmp:
        box = Path(tmp)
        (box / "clients").mkdir()
        for fixture in FIXTURES.iterdir():
            if fixture.is_file():
                shutil.copyfile(fixture, box / "clients" / fixture.name)
        path = box / "frozen-v1.json"
        path.write_text(json.dumps(record), encoding="utf-8")
        yield path


def measured_turns(record, client):
    """Return the turn numbers the client's rows use."""
    turns = set()
    prefix = f"client/{client}/turn/"
    for row in record["rows"]:
        row_id = row["row"]
        if row_id.startswith(prefix):
            turns.add(int(row_id.split("/")[3]))
    return turns


class SemanticsTests(unittest.TestCase):
    def test_turns_statement_covers_every_measured_turn(self):
        record = load_record()
        self.assertEqual(baseline_validate.validate(RECORD), [])
        for client in record["clients"]:
            declared = {item["turn"] for item in client["turns"]}
            self.assertEqual(declared, measured_turns(record, client["client"]))

        def drop_turn(record):
            record["clients"][1]["turns"] = record["clients"][1]["turns"][:1]

        with mutated_record(drop_turn) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC3" in error and "turn numbers do not match" in error for error in errors),
            errors,
        )

    def test_turns_statement_requires_delivers_and_cumulative(self):
        def blank_delivers(record):
            record["clients"][0]["turns"][0]["delivers"] = ""

        with mutated_record(blank_delivers) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC3" in error and "delivers" in error for error in errors), errors
        )

        def bad_cumulative(record):
            record["clients"][0]["turns"][0]["cumulative"] = "yes"

        with mutated_record(bad_cumulative) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC3" in error and "cumulative" in error for error in errors), errors
        )

    def test_ranking_names_its_tie_break(self):
        def blank_tie_break(record):
            record["per_tool_ranking"]["tie_break"] = ""

        with mutated_record(blank_tie_break) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC4" in error and "tie_break" in error for error in errors), errors
        )

    def test_unverified_entry_names_how_the_gap_closes(self):
        def blank_closes_by(record):
            record["unobserved"][0]["closes_by"] = ""

        with mutated_record(blank_closes_by) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC5" in error and "closes_by" in error for error in errors), errors
        )


    def test_measured_row_declares_whether_it_is_recomputable(self):
        def drop_flag(record):
            record["rows"][0].pop("recomputable")

        with mutated_record(drop_flag) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC8" in error and "recomputable" in error for error in errors), errors
        )

    def test_non_recomputable_row_needs_a_resolvable_receipt(self):
        def unknown_receipt(record):
            record["rows"][-1]["receipt_id"] = "missing-receipt"

        with mutated_record(unknown_receipt) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC8" in error and "receipt_id" in error for error in errors), errors
        )

        def wrong_version(record):
            record["receipts"]["pinned-local-run-2026-11-05"]["version"] = "9.9.9"

        with mutated_record(wrong_version) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any("AC8" in error and "version" in error for error in errors), errors
        )

    def test_path_guard_refuses_a_relative_private_segment(self):
        def private_segment(record):
            record["unobserved"][0]["reason"] = "private/work/notes"

        with mutated_record(private_segment) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("path boundary" in error for error in errors), errors)

    def test_activated_row_records_the_input_its_count_came_from(self):
        record = load_record()
        row = activated(record)
        self.assertIs(row["recomputable"], True)
        self.assertEqual(row["input_sha256"], activated_row.INPUT_SHA256)
        self.assertEqual(
            hashlib.sha256((HERE.parents[1] / row["file"]).read_bytes()).hexdigest(),
            row["input_sha256"],
        )
        receipt = record["receipts"][row["receipt_id"]]
        self.assertIs(receipt["recomputable_from_public_inputs"], True)
        self.assertEqual(baseline_validate.validate(RECORD), [])

    def test_activated_row_cannot_claim_it_is_not_recomputable(self):
        def deny_recomputable(record):
            activated(record)["recomputable"] = False

        errors = refusals(deny_recomputable)
        wanted = [error for error in errors if ACTIVATED_ROW in error and "recomputable" in error]
        self.assertTrue(wanted, errors)

        def drop_input_digest(record):
            activated(record).pop("input_sha256")

        errors = refusals(drop_input_digest)
        wanted = [error for error in errors if ACTIVATED_ROW in error and "input_sha256" in error]
        self.assertTrue(wanted, errors)

    def test_activated_row_records_the_expected_input_digest(self):
        for value in ("0" * 64, activated_row.INPUT_SHA256[:-1] + "0", ""):
            with self.subTest(input_sha256=value):
                def other_digest(record, value=value):
                    activated(record)["input_sha256"] = value

                errors = refusals(other_digest)
                wanted = [
                    error for error in errors if ACTIVATED_ROW in error and "input_sha256" in error
                ]
                self.assertTrue(wanted, errors)

    def test_activated_receipt_must_state_public_recomputation(self):
        def deny_public_recomputation(record):
            row = activated(record)
            record["receipts"][row["receipt_id"]]["recomputable_from_public_inputs"] = False

        errors = refusals(deny_public_recomputation)
        wanted = [error for error in errors if ACTIVATED_ROW in error and "receipt" in error]
        self.assertTrue(wanted, errors)

        def drop_receipts(record):
            record["receipts"] = {}

        errors = refusals(drop_receipts)
        wanted = [error for error in errors if ACTIVATED_ROW in error and "receipt" in error]
        self.assertTrue(wanted, errors)


if __name__ == "__main__":
    unittest.main()
