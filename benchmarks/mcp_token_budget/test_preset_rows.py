#!/usr/bin/env python3
"""Tests for the four measured preset rows and the packs they were measured from.

Every byte count, digest, and token count is recomputed here from the committed
pack, so a moved pack, a changed canonicalizer, or a stale row fails. The live
test re-runs the recorder against a built binary inside a temporary directory, so
the checks are never satisfied by re-reading what the recorder already wrote.
That live comparison is skipped, naming the catalog drift, once the live ``all``
row's ``catalog_hash`` leaves the recorded one: the record is frozen, so later
measurements belong in a separate record file.
"""

import copy
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import baseline_validate
import offline_tokenizer

RECORDER = HERE / "preset_rows.py"
PRESETS_DIR = HERE / "presets"
RECORD = PRESETS_DIR / "frozen-v1.json"
PACKS = PRESETS_DIR / "packs"
REPO_ROOT = HERE.parents[1]

EXPECTED_PRESETS = ["minimal", "coding", "ops", "all"]
FULL_PRESET = "all"
EAGER_CLIENT = "synthetic-eager-client"
EAGER_ROW = "client/synthetic-eager-client/turn/1"
ON_DEMAND_CLIENT = "synthetic-on-demand-client"
MEASURED_LABEL = "Measured"
ISO_DATE = re.compile(r"\d{4}-\d{2}-\d{2}\Z")
SHA256_HEX = re.compile(r"[0-9a-f]{64}\Z")


def load_record(path=RECORD):
    """Return the record object at ``path``."""
    return json.loads(Path(path).read_text(encoding="utf-8"))


def load_tools(preset, packs=PACKS):
    """Return the tools array of the pack committed for ``preset``."""
    pack = json.loads((Path(packs) / f"{preset}.json").read_text(encoding="utf-8"))
    return pack["tools"]


def canonical(tools):
    """Serialize a tools array to the bytes one preset row measures."""
    return json.dumps(
        tools, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def byte_failures(rows, packs=PACKS):
    """Return one message per row whose bytes or sha256 disagree with its pack."""
    failures = []
    for row in rows:
        measured = canonical(load_tools(row["preset"], packs))
        digest = hashlib.sha256(measured).hexdigest()
        if row["bytes"] != len(measured):
            failures.append(
                f"{row['preset']}: row bytes {row['bytes']} != pack bytes {len(measured)}"
            )
        if row["input_sha256"] != digest:
            failures.append(
                f"{row['preset']}: row sha256 {row['input_sha256']} != pack sha256 {digest}"
            )
    return failures


def token_failures(rows, tokenizer, packs=PACKS):
    """Return one message per row whose token count disagrees with its pack."""
    failures = []
    for row in rows:
        text = canonical(load_tools(row["preset"], packs)).decode("utf-8")
        expected = tokenizer.count(text)
        if row["tokens"] != expected:
            failures.append(
                f"{row['preset']}: row tokens {row['tokens']} != pack tokens {expected}"
            )
    return failures


def strip_dates(rows):
    """Return the rows without the measurement date, which a live run moves."""
    return [
        {key: value for key, value in row.items() if key != "date"} for row in rows
    ]


def pack_digests(packs=PACKS):
    """Return the sha256 of each committed pack, keyed by preset."""
    return {
        preset: hashlib.sha256((Path(packs) / f"{preset}.json").read_bytes()).hexdigest()
        for preset in EXPECTED_PRESETS
    }


def catalog_hashes(rows):
    """Return each row's recorded catalog hash, keyed by preset."""
    return {row["preset"]: row["catalog_hash"] for row in rows}


def catalog_drift_reason(record, live):
    """Return why a live export cannot be compared with ``record``, or None."""
    live_hash = catalog_hashes(live["rows"])[FULL_PRESET]
    recorded_hash = catalog_hashes(record["rows"])[FULL_PRESET]
    if live_hash == recorded_hash:
        return None
    return (
        "the catalog moved since the measurement: the live 'all' row carries "
        f"{live_hash}, the recorded 'all' row carries {recorded_hash}; later "
        "measurements belong in a separate record file"
    )


def live_mismatches(record, live, live_packs):
    """Return one message per disagreement between a live export and ``record``."""
    failures = []
    if strip_dates(live["rows"]) != strip_dates(record["rows"]):
        failures.append("the live rows disagree with the recorded rows")
    live_measurement = {
        key: value for key, value in live["measurement"].items() if key != "command"
    }
    if live_measurement != {
        key: value for key, value in record["measurement"].items() if key != "command"
    }:
        failures.append("the live measurement disagrees with the record")
    for key in ("delivery", "delivery_note", "baseline_row", "baseline_record"):
        if live[key] != record[key]:
            failures.append(f"the live {key} disagrees with the record")
    for preset in EXPECTED_PRESETS:
        if (Path(live_packs) / f"{preset}.json").read_bytes() != (
            PACKS / f"{preset}.json"
        ).read_bytes():
            failures.append(f"{preset}: the live pack differs from the committed pack")
    return failures


def find_binary():
    """Return the CLI binary the recorder would use, or None when none is built."""
    configured = os.environ.get("CLIO_BIN")
    candidates = [Path(configured)] if configured else []
    candidates.append(REPO_ROOT / "target" / "release" / "clio")
    candidates.append(REPO_ROOT / "target" / "debug" / "clio")
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    return None


def require_tokenizer(test):
    """Return the offline tokenizer, or skip ``test`` naming what is missing."""
    try:
        return offline_tokenizer.load()
    except offline_tokenizer.BudgetError as exc:
        test.skipTest(
            "the token counts cannot be recomputed without the pinned tokenizer: "
            f"{exc}; install tiktoken and put its site-packages on PYTHONPATH"
        )


class PresetSurfaceTests(unittest.TestCase):
    def test_the_record_holds_four_rows_in_preset_order_and_is_frozen(self):
        record = load_record()
        self.assertEqual(record["format"], "am_mcp_preset_surface")
        self.assertIs(record["frozen"], True)
        self.assertEqual([row["preset"] for row in record["rows"]], EXPECTED_PRESETS)
        self.assertEqual(record["unobserved"], [])

    def test_every_row_is_the_eager_client_turn_and_never_the_on_demand_client(self):
        record = load_record()
        self.assertEqual(record["delivery"], "eager")
        self.assertEqual(record["baseline_row"], EAGER_ROW)
        for row in record["rows"]:
            self.assertEqual(row["baseline_row"], EAGER_ROW)
            self.assertNotIn(ON_DEMAND_CLIENT, json.dumps(row, sort_keys=True))
            self.assertNotIn("discovery-on", json.dumps(row, sort_keys=True))
        note = record["delivery_note"]
        self.assertIn(record["baseline_record"]["path"], note)
        self.assertIn(EAGER_CLIENT, note)
        self.assertIn("discovery-off", note)
        self.assertIn(ON_DEMAND_CLIENT, note)
        self.assertIn("discovery-on", note)

    def test_every_row_carries_one_measured_label_an_iso_date_and_a_sha256(self):
        for row in load_record()["rows"]:
            self.assertIn(row["label"], baseline_validate.LABELS)
            self.assertEqual(row["label"], MEASURED_LABEL)
            self.assertRegex(row["date"], ISO_DATE)
            self.assertRegex(row["input_sha256"], SHA256_HEX)
            self.assertIsInstance(row["bytes"], int)
            self.assertIsInstance(row["tokens"], int)
            self.assertGreater(row["bytes"], 0)
            self.assertGreater(row["tokens"], 0)

    def test_every_row_carries_the_catalog_hash_of_its_own_pack(self):
        record = load_record()
        self.assertNotIn("catalog_hash", record["measurement"])
        recorded = catalog_hashes(record["rows"])
        for preset in EXPECTED_PRESETS:
            pack = json.loads((PACKS / f"{preset}.json").read_text(encoding="utf-8"))
            self.assertEqual(recorded[preset], pack["catalog_hash"], preset)

    def test_bytes_and_sha256_recompute_from_the_committed_pack(self):
        rows = load_record()["rows"]
        for preset in EXPECTED_PRESETS:
            self.assertTrue((PACKS / f"{preset}.json").is_file(), preset)
        self.assertEqual(byte_failures(rows), [])

    def test_tokens_recompute_from_the_committed_pack(self):
        tokenizer = require_tokenizer(self)
        rows = load_record()["rows"]
        self.assertEqual(token_failures(rows, tokenizer), [])
        by_preset = {row["preset"]: row for row in rows}
        self.assertEqual(
            by_preset["minimal"]["tokens"], min(row["tokens"] for row in rows)
        )
        self.assertEqual(by_preset["all"]["tokens"], max(row["tokens"] for row in rows))

    def test_minimal_is_the_smallest_preset_and_all_the_largest(self):
        rows = load_record()["rows"]
        by_preset = {row["preset"]: row for row in rows}
        self.assertEqual(
            by_preset["minimal"]["bytes"], min(row["bytes"] for row in rows)
        )
        self.assertEqual(by_preset["all"]["bytes"], max(row["bytes"] for row in rows))

    def test_ops_preset_lists_more_tools_than_coding(self):
        self.assertGreater(len(load_tools("ops")), len(load_tools("coding")))

    def test_the_baseline_digest_matches_the_file_the_record_names(self):
        baseline = load_record()["baseline_record"]
        path = (REPO_ROOT / baseline["path"]).resolve()
        self.assertTrue(path.is_relative_to(REPO_ROOT), baseline["path"])
        self.assertTrue(path.is_file(), baseline["path"])
        self.assertEqual(
            hashlib.sha256(path.read_bytes()).hexdigest(), baseline["sha256"]
        )

    def test_the_live_export_reproduces_the_record_and_packs(self):
        binary = find_binary()
        if binary is None:
            self.skipTest(
                "no clio binary to export with: set CLIO_BIN or build "
                "target/release/clio to compare the live surface with the record"
            )
        require_tokenizer(self)
        before = pack_digests()
        with tempfile.TemporaryDirectory(prefix="clio-preset-live-") as tmp:
            box = Path(tmp)
            out = box / "frozen-v1.json"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(RECORDER),
                    "--bin",
                    str(binary),
                    "--out",
                    str(out),
                ],
                cwd=str(box),
                capture_output=True,
                env=dict(os.environ),
                check=False,
            )
            self.assertEqual(
                completed.returncode, 0, completed.stderr.decode("utf-8", "replace")
            )
            self.assertEqual(
                sorted(path.name for path in box.iterdir()), ["frozen-v1.json", "packs"]
            )
            live = load_record(out)
            committed = load_record()
            reason = catalog_drift_reason(committed, live)
            if reason is not None:
                self.skipTest(reason)
            self.assertEqual(live_mismatches(committed, live, box / "packs"), [])
            self.assertEqual(
                sorted(path.name for path in (box / "packs").iterdir()),
                sorted(f"{preset}.json" for preset in EXPECTED_PRESETS),
            )
        self.assertEqual(pack_digests(), before)


class RecomputationGuardTests(unittest.TestCase):
    def test_a_mutated_byte_count_is_reported_as_a_mismatch(self):
        rows = copy.deepcopy(load_record()["rows"])
        self.assertEqual(byte_failures(rows), [])
        rows[1]["bytes"] += 1
        failures = byte_failures(rows)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("coding", failures[0])
        self.assertIn(f"row bytes {rows[1]['bytes']}", failures[0])
        self.assertIn(f"pack bytes {rows[1]['bytes'] - 1}", failures[0])


if __name__ == "__main__":
    unittest.main()
