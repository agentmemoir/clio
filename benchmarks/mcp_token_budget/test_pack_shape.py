#!/usr/bin/env python3
"""Tests for the pack-shape probe, the choice, and the one record it writes.

The probe is driven against the committed baseline captures, so a status is
always the status today's evidence supports. The record is read from disk and
held to the shipped pack file's own digest, computed with the same canonical
serialization the budget gate uses and with no tokenizer, so the assertions stay
hermetic.

A capture can refute client support for shared schema references but cannot
establish it, so the probe tests below hold every capture path to `not_run` or
`failed` and assert that no baseline fixture can produce `passed`.
"""

import contextlib
import hashlib
import io
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
ROOT = HERE.parents[1]

import baseline_provenance
import pack_shape
import probe_pack_shape

RECORD = HERE / "pack_shape.json"
SHIPPED_PACK = ROOT / pack_shape.CANDIDATE_PATHS[1][1]
REFERENCE_PACK = ROOT / pack_shape.CANDIDATE_PATHS[0][1]
PROBE = HERE / "probe_pack_shape.py"
CLIENTS = HERE / "baseline" / "clients"

REFUTE_ONLY = "cannot establish"


def load_record():
    """Return the committed shape record."""
    return json.loads(RECORD.read_text(encoding="utf-8"))


def canonical_digest(path):
    """Return the gate's tools digest for the pack file at ``path``."""
    pack = json.loads(Path(path).read_text(encoding="utf-8"))
    return hashlib.sha256(baseline_provenance.canonical_tools(pack["tools"])).hexdigest()


def references(schema):
    """Return every local `$ref` string inside one schema document."""
    found = []
    stack = [schema]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            reference = node.get("$ref")
            if isinstance(reference, str) and reference.startswith("#"):
                found.append(reference)
            stack.extend(node.values())
        elif isinstance(node, list):
            stack.extend(node)
    return found


def resolves(schema, reference):
    """Whether ``reference`` resolves inside ``schema`` alone."""
    path = reference.lstrip("#").lstrip("/")
    node = schema
    for raw in path.split("/") if path else []:
        token = raw.replace("~1", "/").replace("~0", "~")
        if isinstance(node, dict) and token in node:
            node = node[token]
        elif isinstance(node, list) and token.isdigit() and int(token) < len(node):
            node = node[int(token)]
        else:
            return False
    return True


def write_baseline(directory, fixture_name, payload=b"{}", sha256=None):
    """Write a baseline record naming one fixture under ``directory``.

    The fixture is written only when ``payload`` is not None, so a caller can
    name a fixture that does not exist.
    """
    clients = Path(directory) / "clients"
    clients.mkdir(exist_ok=True)
    if payload is not None:
        (clients / fixture_name).write_bytes(payload)
        sha256 = sha256 or hashlib.sha256(payload).hexdigest()
    record = {
        "clients": [
            {
                "client": "fixture-client",
                "client_version": "9.9.9",
                "capture": "synthetic-replay",
                "fixture": {
                    "name": fixture_name,
                    "version": 1,
                    "sha256": sha256 or "0" * 64,
                },
            }
        ]
    }
    path = Path(directory) / "frozen-v1.json"
    path.write_text(json.dumps(record), encoding="utf-8")
    return path


def run_probe(*args):
    """Run the probe command and return its completed process."""
    return subprocess.run(
        [sys.executable, str(PROBE), *args],
        cwd=str(HERE),
        capture_output=True,
        text=True,
        check=False,
    )


class ProbeTest(unittest.TestCase):
    """The probe reports only what the committed captures show."""

    def test_todays_captures_leave_client_support_unobserved(self):
        receipt = probe_pack_shape.probe()
        self.assertEqual(receipt["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertIn("no shared schema reference", receipt["reason"])
        self.assertIn(REFUTE_ONLY, receipt["reason"])

    def test_the_receipt_names_the_captured_client_fixture(self):
        receipt = probe_pack_shape.probe()
        client = receipt["client"]
        self.assertFalse(client["fixture"].startswith("/"))
        self.assertIn("baseline/clients/", client["fixture"])
        self.assertEqual(client["name"], "synthetic-eager-client")
        self.assertEqual(client["version"], "1.0.0")
        self.assertEqual(client["provenance"], probe_pack_shape.PROVENANCE_SYNTHETIC)
        digest = hashlib.sha256(
            (ROOT / client["fixture"]).read_bytes()
        ).hexdigest()
        self.assertEqual(client["sha256"], digest)

    def test_the_receipt_states_only_observed_counts(self):
        receipt = probe_pack_shape.probe()
        for key in ("refs_observed", "defs_observed", "unresolved_refs", "stripped_schemas"):
            self.assertIsInstance(receipt[key], int)
            self.assertEqual(receipt[key], 0)
        self.assertEqual(receipt["receipt_id"], probe_pack_shape.receipt_id(
            receipt["client"], receipt["status"]
        ))

    def test_a_capture_with_resolved_references_is_observed(self):
        payload = {
            "tools": [
                {"inputSchema": {"$defs": {"x": {"type": "string"}},
                                 "properties": {"a": {"$ref": "#/$defs/x"}}}}
            ]
        }
        observed = probe_pack_shape._observe(payload)
        self.assertEqual(observed, {"refs_observed": 1, "defs_observed": 1, "unresolved_refs": 0})
        self.assertEqual(probe_pack_shape._stripped(payload), 0)

    def test_a_capture_with_a_dangling_reference_fails(self):
        payload = {"inputSchema": {"properties": {"a": {"$ref": "#/$defs/missing"}}}}
        observed = probe_pack_shape._observe(payload)
        self.assertEqual(observed["refs_observed"], 1)
        self.assertEqual(observed["unresolved_refs"], 1)

    def test_an_external_reference_is_never_observed_as_local(self):
        payload = {"inputSchema": {"$ref": "https://example.test/schema.json"}}
        self.assertEqual(probe_pack_shape._observe(payload)["refs_observed"], 0)

    def test_resolved_local_references_never_pass(self):
        payload = {
            "tools": [
                {"inputSchema": {"$defs": {"x": {"type": "string"}},
                                 "properties": {"a": {"$ref": "#/$defs/x"}}}}
            ]
        }
        status, reason = probe_pack_shape._status(probe_pack_shape._observe(payload))
        self.assertEqual(status, probe_pack_shape.STATUS_NOT_RUN)
        self.assertIn("every one resolves", reason)
        self.assertIn(REFUTE_ONLY, reason)

    def test_a_schema_stripped_of_its_content_fails(self):
        payload = {"tools": [{"inputSchema": {}}, {"outputSchema": None}]}
        self.assertEqual(probe_pack_shape._stripped(payload), 2)
        status, reason = probe_pack_shape._status(
            probe_pack_shape._observe(payload), probe_pack_shape._stripped(payload)
        )
        self.assertEqual(status, probe_pack_shape.STATUS_FAILED)
        self.assertIn("stripped", reason)

    def test_a_probe_of_a_capture_with_resolved_references_is_not_run(self):
        payload = json.dumps(
            {"tools": [{"inputSchema": {"$defs": {"x": {"type": "string"}},
                                        "properties": {"a": {"$ref": "#/$defs/x"}}}}]}
        ).encode("utf-8")
        with tempfile.TemporaryDirectory() as directory:
            receipt = probe_pack_shape.probe(
                write_baseline(directory, "resolved.json", payload)
            )
        self.assertEqual(receipt["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertEqual(receipt["refs_observed"], 1)
        self.assertEqual(receipt["unresolved_refs"], 0)
        self.assertIn(REFUTE_ONLY, receipt["reason"])

    def test_no_baseline_fixture_can_yield_passed(self):
        fixtures = sorted(CLIENTS.glob("*.json"))
        self.assertTrue(fixtures)
        for path in fixtures:
            payload = json.loads(path.read_text(encoding="utf-8"))
            status, _ = probe_pack_shape._status(
                probe_pack_shape._observe(payload),
                probe_pack_shape._stripped(payload),
            )
            self.assertNotEqual(status, probe_pack_shape.STATUS_PASSED, path.name)
        self.assertNotEqual(
            probe_pack_shape.probe()["status"], probe_pack_shape.STATUS_PASSED
        )

    def test_passed_stays_an_allowed_status(self):
        self.assertIn(probe_pack_shape.STATUS_PASSED, probe_pack_shape.STATUSES)
        self.assertEqual(
            pack_shape.choose(probe_pack_shape.STATUS_PASSED), "reference"
        )

    def test_a_missing_capture_yields_a_not_run_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            path = write_baseline(directory, "absent.json", payload=None)
            receipt = probe_pack_shape.probe(path)
        self.assertEqual(receipt["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertFalse(receipt["capture_available"])
        self.assertIn("absent.json", receipt["reason"])
        self.assertIn("unavailable", receipt["reason"])
        self.assertEqual(receipt["client"]["name"], "fixture-client")
        self.assertEqual(receipt["client"]["version"], "9.9.9")
        self.assertEqual(receipt["receipt_id"], probe_pack_shape.receipt_id(
            receipt["client"], receipt["status"]
        ))

    def test_a_digest_mismatch_yields_a_not_run_receipt(self):
        payload = b'{"tools": []}'
        with tempfile.TemporaryDirectory() as directory:
            path = write_baseline(directory, "capture.json", payload, sha256="ab" * 32)
            receipt = probe_pack_shape.probe(path)
        self.assertEqual(receipt["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertIn("digest", receipt["reason"])
        self.assertIn("ab" * 32, receipt["reason"])
        self.assertIn("capture.json", receipt["reason"])

    def test_a_missing_baseline_record_yields_a_not_run_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            receipt = probe_pack_shape.probe(Path(directory) / "absent.json")
        self.assertEqual(receipt["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertIsNone(receipt["client"])
        self.assertIn(probe_pack_shape.UNKNOWN_CLIENT, receipt["receipt_id"])

    def test_the_receipt_is_deterministic(self):
        first = probe_pack_shape.probe()
        second = probe_pack_shape.probe()
        self.assertEqual(first, second)
        self.assertEqual(
            probe_pack_shape.render(first), probe_pack_shape.render(second)
        )
        self.assertRegex(
            first["receipt_id"], re.compile(r"^pack-shape-probe-[0-9a-f]{16}-not_run$")
        )
        self.assertEqual(first["capture_available"], True)


class CommandTest(unittest.TestCase):
    """The commands are pipeable, deterministic, and never crash on a capture."""

    def test_json_stdout_is_only_the_receipt(self):
        result = run_probe("--json")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stderr, "")
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertEqual(result.stdout, probe_pack_shape.render(receipt))

    def test_two_probe_runs_print_the_same_receipt(self):
        self.assertEqual(run_probe("--json").stdout, run_probe("--json").stdout)

    def test_an_unavailable_capture_exits_zero_with_a_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            path = write_baseline(directory, "absent.json", payload=None)
            result = run_probe("--baseline", str(path), "--json")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stderr, "")
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertIn("absent.json", receipt["reason"])

    def test_pack_shape_still_writes_a_record_for_an_unavailable_capture(self):
        before = RECORD.read_bytes()
        with tempfile.TemporaryDirectory() as directory:
            path = write_baseline(directory, "absent.json", payload=None)
            out = Path(directory) / "record.json"
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = pack_shape.main(
                    ["--baseline", str(path), "--out", str(out)]
                )
            self.assertEqual(code, 0)
            self.assertTrue(out.is_file())
            record = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(record["probe"]["status"], probe_pack_shape.STATUS_NOT_RUN)
        self.assertIn("absent.json", record["probe"]["reason"])
        self.assertEqual(record["shipped"]["shape"], pack_shape.crate_shape())
        self.assertEqual(record["shipped"]["shape"], pack_shape.SHAPE_FLATTENED)
        self.assertEqual(RECORD.read_bytes(), before)


class ChooseTest(unittest.TestCase):
    """Only an observed pass ships the reference shape."""

    def test_a_pass_ships_the_reference_shape(self):
        self.assertEqual(pack_shape.choose(probe_pack_shape.STATUS_PASSED), "reference")

    def test_a_failure_ships_the_flattened_shape(self):
        self.assertEqual(pack_shape.choose(probe_pack_shape.STATUS_FAILED), "flattened")

    def test_an_unrun_probe_ships_the_flattened_shape(self):
        self.assertEqual(pack_shape.choose(probe_pack_shape.STATUS_NOT_RUN), "flattened")

    def test_an_unknown_status_is_refused(self):
        with self.assertRaises(pack_shape.PackShapeError):
            pack_shape.choose("maybe")

    def test_the_crate_ships_one_of_the_two_candidate_shapes(self):
        self.assertIn(pack_shape.crate_shape(), pack_shape.SHAPES)


class RecordTest(unittest.TestCase):
    """The record is written once and ties the choice to the shipped pack."""

    def test_exactly_one_record_exists(self):
        records = sorted(HERE.glob("pack_shape*.json"))
        self.assertEqual([path.name for path in records], [RECORD.name])

    def test_the_record_names_the_shipped_shape_the_crate_emits(self):
        record = load_record()
        self.assertEqual(record["shipped"]["shape"], pack_shape.crate_shape())
        self.assertEqual(record["shipped"]["shape"], pack_shape.choose(
            record["probe"]["status"]
        ))

    def test_both_candidates_are_measured(self):
        record = load_record()
        self.assertEqual(
            [candidate["shape"] for candidate in record["candidates"]],
            list(pack_shape.SHAPES),
        )
        for candidate in record["candidates"]:
            self.assertTrue((ROOT / candidate["path"]).is_file(), candidate["path"])
            for key in ("bytes", "tokens", "sha256", "path", "shape"):
                self.assertIn(key, candidate)
            self.assertGreater(candidate["bytes"], 0)
            self.assertGreater(candidate["tokens"], 0)
            self.assertEqual(candidate["sha256"], canonical_digest(ROOT / candidate["path"]))

    def test_the_shipped_digest_is_the_shipped_pack_file_digest(self):
        record = load_record()
        self.assertEqual(record["shipped"]["path"], pack_shape.CANDIDATE_PATHS[1][1])
        self.assertEqual(record["shipped"]["sha256"], canonical_digest(SHIPPED_PACK))

    def test_the_shipped_shape_is_the_flattened_candidate(self):
        record = load_record()
        self.assertEqual(record["shipped"]["shape"], "flattened")
        shipped = next(
            candidate for candidate in record["candidates"] if candidate["shape"] == "flattened"
        )
        self.assertEqual(record["shipped"]["sha256"], shipped["sha256"])

    def test_the_probe_status_is_one_of_three(self):
        record = load_record()
        self.assertIn(record["probe"]["status"], probe_pack_shape.STATUSES)
        if record["probe"]["status"] != probe_pack_shape.STATUS_PASSED:
            self.assertEqual(record["shipped"]["shape"], "flattened")

    def test_the_recorded_probe_is_not_a_capture_pass(self):
        record = load_record()
        self.assertNotEqual(record["probe"]["status"], probe_pack_shape.STATUS_PASSED)
        self.assertIn(REFUTE_ONLY, record["probe"]["reason"])

    def test_each_candidate_keeps_its_own_resolvable_references(self):
        for path in (REFERENCE_PACK, SHIPPED_PACK):
            pack = json.loads(path.read_text(encoding="utf-8"))
            for tool in pack["tools"]:
                for key in ("inputSchema", "outputSchema"):
                    schema = tool.get(key)
                    if not isinstance(schema, dict):
                        continue
                    for reference in references(schema):
                        self.assertTrue(resolves(schema, reference), f"{path.name} {reference}")
        reference = json.loads(REFERENCE_PACK.read_text(encoding="utf-8"))
        shipped = json.loads(SHIPPED_PACK.read_text(encoding="utf-8"))
        self.assertGreater(len(references(reference)), 0)
        self.assertEqual(len(references(shipped)), 0)

    def test_a_record_of_another_shape_is_never_rewritten(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "pack_shape.json"
            path.write_text(json.dumps({"shipped": {"shape": "reference"}}), encoding="utf-8")
            with self.assertRaises(pack_shape.PackShapeError):
                pack_shape.write_record(load_record(), path)
            self.assertEqual(
                json.loads(path.read_text(encoding="utf-8"))["shipped"]["shape"], "reference"
            )


if __name__ == "__main__":
    unittest.main()
