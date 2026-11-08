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

import activated_row
import baseline_replay
import baseline_validate
import offline_tokenizer

RECORD = HERE / "baseline" / "frozen-v1.json"
FIXTURES = HERE / "baseline" / "clients"

#: Digest of the committed record file. An edit or an append changes this value.
FROZEN_V1_SHA256 = "484b46ac9ee2703920ee7f21f7f1eb16a00feca36336269489bf10e718e3be40"

LABELS = frozenset(
    {"Measured", "Vendor-measured", "External", "Inferred", "Decision", "Unverified"}
)
RETIRED_SHA256 = "51ec302b11becd64c388c25b88495a95bcf7705ba06142519a275c0b76acba80"
ACTIVATED_ROW = baseline_validate.ACTIVATED_ROW
ACTIVATED_SHA256 = activated_row.CONTENT_SHA256


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


def refusals(mutate):
    """Return the validator errors for one mutation of the real record."""
    with mutated_record(mutate) as path:
        return baseline_validate.validate(path)


def changed_field(row_id, key, value):
    """Return the validator errors after one row field is changed."""
    def change(record):
        row_by_id(record, row_id)[key] = value

    return refusals(change)


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

        errors = refusals(blank_version)
        self.assertTrue(any("AC1" in error for error in errors), errors)

        def bad_delivery(record):
            record["clients"][0]["delivery"] = "later"

        errors = refusals(bad_delivery)
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
                hashlib.sha256(path.read_bytes()).hexdigest(), client["fixture"]["sha256"]
            )
            fixtures[client["delivery"]] = fixture
            for turn in (entry["turn"] for entry in client["turns"]):
                for unit in ("bytes", "tokens"):
                    row = row_by_id(record, f"client/{client['client']}/turn/{turn}/{unit}")
                    self.assertEqual(row["label"], "Measured")
        self.assertEqual(set(fixtures), {"eager", "on_demand"})

        def bad_digest(record):
            record["clients"][0]["fixture"]["sha256"] = "not-a-digest"

        errors = refusals(bad_digest)
        self.assertTrue(any("AC2" in error for error in errors), errors)

    def test_ac2_wrong_fixture_digest_is_refused(self):
        def zero_digest(record):
            record["clients"][0]["fixture"]["sha256"] = "0" * 64

        errors = refusals(zero_digest)
        self.assertTrue(any("AC2" in error and "fixture sha256" in error for error in errors), errors)

    def test_ac3_each_turn_has_separate_bytes_and_tokens_rows(self):
        record = load_record()
        for client in record["clients"]:
            for turn in (entry["turn"] for entry in client["turns"]):
                for unit in ("bytes", "tokens"):
                    wanted_id = f"client/{client['client']}/turn/{turn}/{unit}"
                    rows = [row for row in record["rows"] if row["row"] == wanted_id]
                    self.assertEqual(len(rows), 1)
                    self.assertIn(rows[0]["label"], LABELS)
                    self.assertEqual(rows[0]["unit"], unit)

        def drop_tokens_row(record):
            dropped = "client/synthetic-eager-client/turn/1/tokens"
            record["rows"] = [row for row in record["rows"] if row["row"] != dropped]

        errors = refusals(drop_tokens_row)
        wanted = [error for error in errors if "AC3" in error and "synthetic-eager-client" in error]
        self.assertTrue(wanted, errors)

    def test_ac3_turn_values_match_recomputed_measurement(self):
        require_tokenizer()
        record = load_record()
        tokenizer = offline_tokenizer.load()
        for client in record["clients"]:
            fixture = load_fixture(client["fixture"]["name"])
            for result in baseline_replay.replay(fixture, tokenizer):
                for unit, key in (("bytes", "canonical_bytes"), ("tokens", "tokens")):
                    row_id = f"client/{client['client']}/turn/{result['turn']}/{unit}"
                    self.assertEqual(row_by_id(record, row_id)["value"], result[key])

        def wrong_bytes(record):
            row_by_id(record, "client/synthetic-eager-client/turn/1/bytes")["value"] += 1

        errors = refusals(wrong_bytes)
        wanted = [error for error in errors if "AC3" in error and "synthetic-eager-client" in error]
        self.assertTrue(any("turn 1" in error for error in wanted), errors)

    def test_ac4_ranking_names_its_sort_figure(self):
        record = load_record()
        ranking = record["per_tool_ranking"]
        self.assertEqual(ranking["sorted_by"], "tool_tokens")
        self.assertTrue(all("tool_tokens" in item for item in ranking["items"]))

        def sort_by_bytes(record):
            record["per_tool_ranking"]["sorted_by"] = "tool_bytes"

        errors = refusals(sort_by_bytes)
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

        errors = refusals(swap_first_two)
        self.assertTrue(any("AC4" in error for error in errors), errors)

    def test_ac5_every_row_has_one_date_and_one_label(self):
        record = load_record()
        for row in record["rows"]:
            self.assertEqual(len([key for key in row if key == "date"]), 1)
            self.assertIn(row["label"], LABELS)
            date.fromisoformat(row["date"])

        def drop_date(record):
            record["rows"][0].pop("date")

        self.assertTrue(any("AC5" in error for error in refusals(drop_date)))

        def bad_label(record):
            record["rows"][0]["label"] = "Guess"

        self.assertTrue(any("AC5" in error for error in refusals(bad_label)))

    def test_ac5_unverified_entry_requires_reason(self):
        record = load_record()
        entry = record["unobserved"][0]
        self.assertEqual(entry["label"], "Unverified")
        self.assertTrue(entry["reason"].strip())
        self.assertNotIn("value", entry)
        date.fromisoformat(entry["date"])

        def blank_reason(record):
            record["unobserved"][0]["reason"] = ""

        errors = refusals(blank_reason)
        self.assertTrue(any("AC5" in error and "reason" in error for error in errors), errors)

        def add_value(record):
            record["unobserved"][0]["value"] = 1

        self.assertTrue(any("AC5" in error for error in refusals(add_value)))

    def test_ac6_record_bytes_are_pinned(self):
        self.assertEqual(hashlib.sha256(RECORD.read_bytes()).hexdigest(), FROZEN_V1_SHA256)
        record = load_record()
        record["rows"].append(copy.deepcopy(record["rows"][0]))
        appended = (json.dumps(record) + "\n").encode("utf-8")
        self.assertNotEqual(hashlib.sha256(appended).hexdigest(), FROZEN_V1_SHA256)

    def test_ac6_validator_rejects_appended_row(self):
        appended_id = "client/synthetic-eager-client/turn/2/bytes"

        def append_row(record):
            record["rows"].append(
                {"row": appended_id, "value": 1, "unit": "bytes", "label": "Measured",
                 "date": "2026-11-05", "input_sha256": "0" * 64}
            )

        errors = refusals(append_row)
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

        errors = changed_field("retired_skill_entry_point/bytes", "content_sha256", "0" * 64)
        self.assertTrue(any("AC7" in error for error in errors), errors)

    def test_ac7_deleting_retired_row_fails_validation(self):
        missing = "retired_skill_entry_point/bytes"

        def delete_row(record):
            record["rows"] = [row for row in record["rows"] if row["row"] != missing]

        errors = refusals(delete_row)
        self.assertTrue(any("AC7" in error and missing in error for error in errors), errors)

    def test_ac7_activated_skill_body_row_is_frozen(self):
        record = load_record()
        row = row_by_id(record, ACTIVATED_ROW)
        self.assertEqual(row["role"], activated_row.ROLE)
        self.assertEqual(row["file"], "skills/clio-mcp/SKILL.md")
        self.assertEqual(row["content_sha256"], ACTIVATED_SHA256)
        self.assertEqual(row["input_sha256"], ACTIVATED_SHA256)
        self.assertIs(row["recomputable"], True)
        self.assertEqual(row["tokenizer_ref"], "pinned")
        self.assertEqual(row["label"], "Measured")
        self.assertEqual(row["unit"], "tokens")
        self.assertGreater(row["value"], 0)
        self.assertEqual(row["factor"], 1.0)
        self.assertIsInstance(row["factor"], float)
        receipt = record["receipts"][row["receipt_id"]]
        self.assertIs(receipt["recomputable_from_public_inputs"], True)

    def test_ac7_deleting_activated_row_fails_validation(self):
        def delete_row(record):
            record["rows"] = [row for row in record["rows"] if row["row"] != ACTIVATED_ROW]

        errors = refusals(delete_row)
        self.assertTrue(any("AC7" in error and ACTIVATED_ROW in error for error in errors), errors)

    def test_ac7_activated_row_fields_and_factor_are_pinned(self):
        cases = (
            ("factor", 1.5),
            ("factor", 1),
            ("factor", "1.0"),
            ("role", "other-role"),
            ("file", "skills/other/SKILL.md"),
            ("content_sha256", "0" * 64),
            ("input_sha256", "0" * 64),
            ("recomputable", False),
        )
        for key, value in cases:
            with self.subTest(field=key, value=value):
                errors = changed_field(ACTIVATED_ROW, key, value)
                wanted = [error for error in errors if ACTIVATED_ROW in error and key in error]
                self.assertTrue(wanted, errors)

    def test_ac7_duplicate_activated_row_fails_validation(self):
        def duplicate(record):
            record["rows"].append(copy.deepcopy(row_by_id(record, ACTIVATED_ROW)))

        errors = refusals(duplicate)
        wanted = [error for error in errors if "exactly one" in error and ACTIVATED_ROW in error]
        self.assertTrue(wanted, errors)

    def test_ac7_retired_rows_keep_their_own_values(self):
        record = load_record()
        self.assertEqual(row_by_id(record, "retired_skill_entry_point/bytes")["value"], 6393)
        self.assertEqual(row_by_id(record, "retired_skill_entry_point/tokens")["value"], 1518)
        for row_id in baseline_validate.RETIRED_ROWS:
            row = row_by_id(record, row_id)
            self.assertEqual(row["content_sha256"], RETIRED_SHA256)
            self.assertNotEqual(row["content_sha256"], ACTIVATED_SHA256)

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
                replayed = baseline_replay.replay(fixture, tokenizer)
                result = next(item for item in replayed if item["turn"] == int(turn))
                self.assertEqual(row["value"], result["tokens"])
            else:
                self.assertEqual(row.get("tokenizer_ref"), "pinned")

        def mark_estimate(record):
            row_by_id(record, "client/synthetic-eager-client/turn/1/tokens")["estimated"] = True

        errors = refusals(mark_estimate)
        self.assertTrue(any("AC8" in error for error in errors), errors)

    def test_ac8_measured_row_without_provenance_is_refused(self):
        def drop_input_digest(record):
            row_by_id(record, "client/synthetic-eager-client/turn/1/bytes").pop("input_sha256")

        wanted = "client/synthetic-eager-client/turn/1/bytes"
        errors = refusals(drop_input_digest)
        self.assertTrue(any("AC8" in error and wanted in error for error in errors), errors)

        def drop_receipt(record):
            row_by_id(record, "retired_skill_entry_point/bytes").pop("receipt_id")

        wanted = "retired_skill_entry_point/bytes"
        errors = refusals(drop_receipt)
        self.assertTrue(any("AC8" in error and wanted in error for error in errors), errors)

    def test_ac8_fallback_estimate_cannot_be_labelled_measured(self):
        for flag in ("estimated", "fallback"):
            with self.subTest(flag=flag):
                errors = changed_field("client/synthetic-eager-client/turn/1/tokens", flag, True)
                self.assertTrue(any("AC8" in error for error in errors), errors)

    def test_ac8_tokenizer_identity_is_enforced(self):
        for key, value in (("version", "9.9.9"), ("implementation", "guesser")):
            def change(record, key=key, value=value):
                record["tokenizer"][key] = value

            errors = refusals(change)
            self.assertTrue(any("AC8" in error and key in error for error in errors), errors)

    def test_pack_identity_is_enforced(self):
        def bad_bytes(record):
            record["pack"]["canonical_bytes"] = 1

        errors = refusals(bad_bytes)
        self.assertTrue(any("AC3" in error and "canonical_bytes" in error for error in errors), errors)

    def test_validator_rejects_absolute_path(self):
        def add_path(record):
            record["unobserved"][0]["reason"] = "/absolute/path"

        errors = refusals(add_path)
        self.assertTrue(any("path boundary" in error for error in errors), errors)

        def add_relative_path(record):
            record["unobserved"][0]["reason"] = "relative/path"

        self.assertEqual(refusals(add_relative_path), [])

    def test_path_guard_refuses_absolute_and_parent_paths(self):
        for marker in ("C:\\data\\file", "folder/../file"):
            def add_path(record, marker=marker):
                record["unobserved"][0]["reason"] = marker

            errors = refusals(add_path)
            self.assertTrue(any("path boundary" in error for error in errors), errors)

    def test_validator_accepts_the_committed_record(self):
        self.assertEqual(baseline_validate.validate(RECORD), [])
        self.assertEqual(baseline_validate.main([str(RECORD)]), 0)


if __name__ == "__main__":
    unittest.main()
