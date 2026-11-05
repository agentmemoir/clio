#!/usr/bin/env python3
"""Tests for the turn statements, ranking tie-break, and unobserved gap.

Each test mutates a copy of the record, copies the fixtures beside it, and
asserts the validator refuses the copy with the expected AC error.
"""

import contextlib
import copy
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import baseline_validate

RECORD = HERE / "baseline" / "frozen-v1.json"
FIXTURES = HERE / "baseline" / "clients"


def load_record():
    return json.loads(RECORD.read_text(encoding="utf-8"))


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


if __name__ == "__main__":
    unittest.main()
