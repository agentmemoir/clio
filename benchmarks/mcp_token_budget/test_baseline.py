#!/usr/bin/env python3
"""Per-criterion tests for the frozen MCP tool-surface baseline.

Every test drives the real record, the real fixtures, and the real validator.
A test that needs the tokenizer skips with a named reason when tiktoken is not
installed, so the hermetic job can run the rest. A test that proves a check can
fail copies the record into a temp directory, mutates the copy, and asserts the
validator names the expected problem.
"""

import contextlib
import copy
import hashlib
import json
import shutil
import sys
import tempfile
import unittest
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import baseline_replay
import baseline_validate
import offline_tokenizer

REPO_ROOT = HERE.parents[1]
RECORD = HERE / "baseline" / "frozen-v1.json"
FIXTURES = HERE / "baseline" / "clients"

#: Digest of the committed record file. An edit or an append changes this value.
FROZEN_V1_SHA256 = "d94fbfa8db884d75baf26da5a921bb2b124eb18f3846c670a76b50b9b8e47e82"

LABELS = frozenset(
    {"Measured", "Vendor-measured", "External", "Inferred", "Decision", "Unverified"}
)
RETIRED_SHA256 = (
    "51ec302b11becd64c388c25b88495a95bcf7705ba06142519a275c0b76acba80"
)


def load_record():
    return json.loads(RECORD.read_text(encoding="utf-8"))


def load_fixture(name):
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


@contextlib.contextmanager
def mutated_record(mutate):
    """Yield a temp record path whose object was changed by ``mutate``."""
    record = copy.deepcopy(load_record())
    mutate(record)
    with tempfile.TemporaryDirectory() as tmp:
        temp_dir = Path(tmp)
        (temp_dir / "clients").mkdir()
        for fixture in FIXTURES.iterdir():
            if fixture.is_file():
                shutil.copyfile(fixture, temp_dir / "clients" / fixture.name)
        path = temp_dir / "frozen-v1.json"
        path.write_text(json.dumps(record), encoding="utf-8")
        yield path


def row_by_id(record, row_id):
    return next(row for row in record["rows"] if row["row"] == row_id)


def require_tokenizer():
    """Skip with a named reason when tiktoken is absent."""
    try:
        import tiktoken  # noqa: F401
    except ImportError as exc:
        raise unittest.SkipTest(f"tiktoken is not installed: {exc}") from None


class BaselineTests(unittest.TestCase):
    def test_ac1_each_client_names_version_and_delivery(self):
        record = load_record()
        self.assertEqual(baseline_validate.validate(RECORD), [])
        deliveries = set()
        for client in record["clients"]:
            self.assertTrue(client["client"].strip())
            self.assertTrue(client["client_version"].strip())
            self.assertIn(client["delivery"], {"eager", "on_demand"})
            deliveries.add(client["delivery"])
        self.assertEqual(deliveries, {"eager", "on_demand"})

        def blank_version(record):
            record["clients"][0]["client_version"] = ""

        with mutated_record(blank_version) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC1" in error for error in errors), errors)

        def bad_delivery(record):
            record["clients"][0]["delivery"] = "later"

        with mutated_record(bad_delivery) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC1" in error for error in errors), errors)

    def test_ac2_eager_and_on_demand_fixtures_exist_and_are_measured(self):
        record = load_record()
        fixtures = {}
        for client in record["clients"]:
            name = client["fixture"]["name"]
            path = FIXTURES / name
            self.assertTrue(path.is_file(), f"missing fixture: {name}")
            fixture = load_fixture(name)
            self.assertEqual(fixture["fixture_format"], "am_mcp_client_replay")
            self.assertEqual(fixture["fixture_version"], 1)
            self.assertEqual(fixture["provenance"], "synthetic-mock")
            self.assertEqual(fixture["delivery"], client["delivery"])
            self.assertEqual(
                hashlib.sha256(path.read_bytes()).hexdigest(),
                client["fixture"]["sha256"],
            )
            fixtures[client["delivery"]] = fixture
            for turn in (entry["turn"] for entry in client["turns"]):
                for unit in ("bytes", "tokens"):
                    row = row_by_id(
                        record, f"client/{client['client']}/turn/{turn}/{unit}"
                    )
                    self.assertEqual(row["label"], "Measured")
        self.assertEqual(set(fixtures), {"eager", "on_demand"})

        def bad_digest(record):
            record["clients"][0]["fixture"]["sha256"] = "not-a-digest"

        with mutated_record(bad_digest) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC2" in error for error in errors), errors)

    def test_ac2_wrong_fixture_digest_is_refused(self):
        def zero_digest(record):
            record["clients"][0]["fixture"]["sha256"] = "0" * 64

        with mutated_record(zero_digest) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC2" in error and "fixture sha256" in error for error in errors), errors)

    def test_ac3_each_turn_has_separate_bytes_and_tokens_rows(self):
        record = load_record()
        for client in record["clients"]:
            for turn in (entry["turn"] for entry in client["turns"]):
                for unit in ("bytes", "tokens"):
                    rows = [
                        row
                        for row in record["rows"]
                        if row["row"] == f"client/{client['client']}/turn/{turn}/{unit}"
                    ]
                    self.assertEqual(len(rows), 1)
                    self.assertIn(rows[0]["label"], LABELS)
                    self.assertEqual(rows[0]["unit"], unit)

        def drop_tokens_row(record):
            record["rows"] = [
                row
                for row in record["rows"]
                if row["row"] != "client/synthetic-eager-client/turn/1/tokens"
            ]

        with mutated_record(drop_tokens_row) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any(
                "AC3" in error and "synthetic-eager-client" in error
                for error in errors
            ),
            errors,
        )

    def test_ac3_turn_values_match_recomputed_measurement(self):
        require_tokenizer()
        record = load_record()
        tokenizer = offline_tokenizer.load()
        for client in record["clients"]:
            fixture = load_fixture(client["fixture"]["name"])
            for result in baseline_replay.replay(fixture, tokenizer):
                for unit, key in (("bytes", "canonical_bytes"), ("tokens", "tokens")):
                    row = row_by_id(
                        record,
                        f"client/{client['client']}/turn/{result['turn']}/{unit}",
                    )
                    self.assertEqual(row["value"], result[key])

        def wrong_bytes(record):
            row_by_id(record, "client/synthetic-eager-client/turn/1/bytes")["value"] += 1

        with mutated_record(wrong_bytes) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(
            any(
                "AC3" in error
                and "synthetic-eager-client" in error
                and "turn 1" in error
                for error in errors
            ),
            errors,
        )

    def test_ac4_ranking_names_its_sort_figure(self):
        record = load_record()
        ranking = record["per_tool_ranking"]
        self.assertEqual(ranking["sorted_by"], "tool_tokens")
        self.assertTrue(all("tool_tokens" in item for item in ranking["items"]))

        def sort_by_bytes(record):
            record["per_tool_ranking"]["sorted_by"] = "tool_bytes"

        with mutated_record(sort_by_bytes) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC4" in error for error in errors), errors)

    def test_ac4_ranking_order_matches_named_figure(self):
        record = load_record()
        items = record["per_tool_ranking"]["items"]
        self.assertEqual(len(items), 10)
        self.assertEqual([item["rank"] for item in items], list(range(1, 11)))
        expected = sorted(items, key=lambda item: (-item["tool_tokens"], item["tool"]))
        self.assertEqual([item["tool"] for item in items], [item["tool"] for item in expected])

        def swap_first_two(record):
            items = record["per_tool_ranking"]["items"]
            items[0], items[1] = items[1], items[0]

        with mutated_record(swap_first_two) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC4" in error for error in errors), errors)

    def test_ac5_every_row_has_one_date_and_one_label(self):
        record = load_record()
        for row in record["rows"]:
            self.assertEqual(len([key for key in row if key == "date"]), 1)
            self.assertIn(row["label"], LABELS)
            date.fromisoformat(row["date"])

        def drop_date(record):
            record["rows"][0].pop("date")

        with mutated_record(drop_date) as path:
            self.assertTrue(any("AC5" in error for error in baseline_validate.validate(path)))

        def bad_label(record):
            record["rows"][0]["label"] = "Guess"

        with mutated_record(bad_label) as path:
            self.assertTrue(any("AC5" in error for error in baseline_validate.validate(path)))

    def test_ac5_unverified_entry_requires_reason(self):
        record = load_record()
        entry = record["unobserved"][0]
        self.assertEqual(entry["label"], "Unverified")
        self.assertTrue(entry["reason"].strip())
        self.assertNotIn("value", entry)
        date.fromisoformat(entry["date"])

        def blank_reason(record):
            record["unobserved"][0]["reason"] = ""

        with mutated_record(blank_reason) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC5" in error and "reason" in error for error in errors), errors)

        def add_value(record):
            record["unobserved"][0]["value"] = 1

        with mutated_record(add_value) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC5" in error for error in errors), errors)

    def test_ac6_record_bytes_are_pinned(self):
        self.assertEqual(
            hashlib.sha256(RECORD.read_bytes()).hexdigest(), FROZEN_V1_SHA256
        )
        record = load_record()
        record["rows"].append(copy.deepcopy(record["rows"][0]))
        appended = (json.dumps(record) + "\n").encode("utf-8")
        self.assertNotEqual(hashlib.sha256(appended).hexdigest(), FROZEN_V1_SHA256)

    def test_ac6_validator_rejects_appended_row(self):
        appended_id = "client/synthetic-eager-client/turn/2/bytes"

        def append_row(record):
            record["rows"].append(
                {
                    "row": appended_id,
                    "value": 1,
                    "unit": "bytes",
                    "label": "Measured",
                    "date": "2026-11-05",
                    "input_sha256": "0" * 64,
                }
            )

        with mutated_record(append_row) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(errors)
        self.assertTrue(any(appended_id in error for error in errors), errors)

    def test_ac7_retired_skill_rows_present_with_digest_and_tokenizer(self):
        record = load_record()
        for row_id in baseline_validate.RETIRED_ROWS:
            rows = [row for row in record["rows"] if row["row"] == row_id]
            self.assertEqual(len(rows), 1, row_id)
            row = rows[0]
            self.assertEqual(row["content_sha256"], RETIRED_SHA256)
            self.assertGreater(row["value"], 0)
            self.assertEqual(row["tokenizer_ref"], "pinned")
            self.assertEqual(row["label"], "Measured")
            self.assertEqual(row["unit"], row_id.rsplit("/", 1)[1])

        def bad_digest(record):
            row_by_id(record, "retired_skill_entry_point/bytes")[
                "content_sha256"
            ] = "0" * 64

        with mutated_record(bad_digest) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC7" in error for error in errors), errors)

    def test_ac7_deleting_retired_row_fails_validation(self):
        missing = "retired_skill_entry_point/bytes"

        def delete_row(record):
            record["rows"] = [row for row in record["rows"] if row["row"] != missing]

        with mutated_record(delete_row) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC7" in error and missing in error for error in errors), errors)

    def test_ac8_measured_token_rows_use_pinned_encoding(self):
        require_tokenizer()
        record = load_record()
        tokenizer = offline_tokenizer.load()
        self.assertEqual(tokenizer.encoding, "cl100k_base")
        self.assertEqual(
            tokenizer.provenance()["asset_sha256"], record["tokenizer"]["asset_sha256"]
        )
        clients = {client["client"]: client for client in record["clients"]}
        for row in record["rows"]:
            if row["unit"] != "tokens" or row["label"] != "Measured":
                continue
            self.assertNotEqual(row.get("estimated"), True)
            self.assertNotEqual(row.get("fallback"), True)
            if row["row"].startswith("client/"):
                _, client_name, _, turn, _ = row["row"].split("/")
                fixture = load_fixture(clients[client_name]["fixture"]["name"])
                result = next(
                    item
                    for item in baseline_replay.replay(fixture, tokenizer)
                    if item["turn"] == int(turn)
                )
                self.assertEqual(row["value"], result["tokens"])
            else:
                self.assertEqual(row.get("tokenizer_ref"), "pinned")

        def mark_estimate(record):
            row_by_id(record, "client/synthetic-eager-client/turn/1/tokens")[
                "estimated"
            ] = True

        with mutated_record(mark_estimate) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC8" in error for error in errors), errors)

    def test_ac8_measured_row_without_provenance_is_refused(self):
        def drop_input_digest(record):
            row_by_id(
                record, "client/synthetic-eager-client/turn/1/bytes"
            ).pop("input_sha256")

        with mutated_record(drop_input_digest) as path:
            errors = baseline_validate.validate(path)
        wanted = "client/synthetic-eager-client/turn/1/bytes"
        self.assertTrue(any("AC8" in error and wanted in error for error in errors), errors)

        def drop_receipt(record):
            row_by_id(
                record, "retired_skill_entry_point/bytes"
            ).pop("receipt_id")

        with mutated_record(drop_receipt) as path:
            errors = baseline_validate.validate(path)
        wanted = "retired_skill_entry_point/bytes"
        self.assertTrue(any("AC8" in error and wanted in error for error in errors), errors)

    def test_ac8_fallback_estimate_cannot_be_labelled_measured(self):
        for flag in ("estimated", "fallback"):
            def add_flag(record, flag=flag):
                row_by_id(record, "client/synthetic-eager-client/turn/1/tokens")[
                    flag
                ] = True

            with mutated_record(add_flag) as path:
                errors = baseline_validate.validate(path)
            self.assertTrue(any("AC8" in error for error in errors), errors)

    def test_ac8_tokenizer_identity_is_enforced(self):
        for key, value in (("version", "9.9.9"), ("implementation", "guesser")):
            def change(record, key=key, value=value):
                record["tokenizer"][key] = value

            with mutated_record(change) as path:
                errors = baseline_validate.validate(path)
            self.assertTrue(any("AC8" in error and key in error for error in errors), errors)

    def test_pack_identity_is_enforced(self):
        def bad_bytes(record):
            record["pack"]["canonical_bytes"] = 1

        with mutated_record(bad_bytes) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("AC3" in error and "canonical_bytes" in error for error in errors), errors)

    def test_validator_rejects_absolute_path(self):
        def add_path(record):
            record["unobserved"][0]["reason"] = "/absolute/path"

        with mutated_record(add_path) as path:
            errors = baseline_validate.validate(path)
        self.assertTrue(any("path boundary" in error for error in errors), errors)

        def add_relative_path(record):
            record["unobserved"][0]["reason"] = "relative/path"

        with mutated_record(add_relative_path) as path:
            self.assertEqual(baseline_validate.validate(path), [])

    def test_path_guard_refuses_absolute_and_parent_paths(self):
        for marker in ("C:\\data\\file", "folder/../file"):
            def add_path(record, marker=marker):
                record["unobserved"][0]["reason"] = marker

            with mutated_record(add_path) as path:
                errors = baseline_validate.validate(path)
            self.assertTrue(any("path boundary" in error for error in errors), errors)

    def test_validator_accepts_the_committed_record(self):
        self.assertEqual(baseline_validate.validate(RECORD), [])
        self.assertEqual(baseline_validate.main([str(RECORD)]), 0)


if __name__ == "__main__":
    unittest.main()
