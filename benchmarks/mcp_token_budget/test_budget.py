"""Tests for the documented MCP token-budget measurement command.

Every test drives `measure.py` as a real subprocess with the real tokenizer and
the real committed encoding asset; nothing about the measurement path is mocked.
The expected byte count is recomputed here from the pack fixture, so a change to
the pack, the canonicalizer, or the tokenizer shows up as a failure.
"""

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import offline_tokenizer

MEASURE = HERE / "measure.py"
PACK = HERE.parents[1] / "crates" / "clio-mcp" / "tests" / "fixtures" / "schema_pack.json"

LABELS = frozenset(
    {"Measured", "Vendor-measured", "External", "Inferred", "Decision", "Unverified"}
)
UNITS = frozenset({"bytes", "tokens"})


def _run(script, pack_path, env=None):
    """Run ``script`` against ``pack_path``; return (exit, stdout_bytes, stderr_text)."""
    child_env = dict(os.environ)
    for key, value in (env or {}).items():
        if value is None:
            child_env.pop(key, None)
        else:
            child_env[key] = value
    completed = subprocess.run(
        [sys.executable, str(script), "--pack", str(pack_path)],
        capture_output=True,
        env=child_env,
        cwd=str(Path(script).resolve().parent),
        check=False,
    )
    return (
        completed.returncode,
        completed.stdout,
        completed.stderr.decode("utf-8", "replace"),
    )


def run_measure(pack_path, env=None):
    """Run the real `measure.py` against ``pack_path``; return (exit, stdout, stderr)."""
    return _run(MEASURE, pack_path, env)


def canonical(pack_text):
    """Recompute the canonical bytes of a pack document, independently of measure.py."""
    return json.dumps(
        json.loads(pack_text), ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")


def copy_measure_tree(destination):
    """Copy measure.py and offline_tokenizer.py, and nothing else, into ``destination``."""
    for name in ("measure.py", "offline_tokenizer.py"):
        shutil.copyfile(HERE / name, Path(destination) / name)
    return Path(destination) / "measure.py"


class MeasureTests(unittest.TestCase):
    def test_two_runs_are_byte_identical(self):
        first_exit, first_out, first_err = run_measure(PACK)
        second_exit, second_out, second_err = run_measure(PACK)
        self.assertEqual(first_exit, 0, first_err)
        self.assertEqual(second_exit, 0, second_err)
        self.assertEqual(first_out, second_out)

    def test_rows_have_value_unit_and_one_label(self):
        code, out, err = run_measure(PACK)
        self.assertEqual(code, 0, err)
        document = json.loads(out.decode("utf-8"))
        rows = document["rows"]
        self.assertEqual(len(rows), 2)
        self.assertEqual([row["row"] for row in rows], ["pack.bytes", "pack.tokens"])
        for row in rows:
            self.assertIsInstance(row["value"], int)
            self.assertGreaterEqual(row["value"], 0)
            self.assertIn(row["unit"], UNITS)
            self.assertIn(row["label"], LABELS)
            self.assertEqual(row["label"], "Measured")
        by_id = {row["row"]: row for row in rows}
        self.assertGreater(by_id["pack.bytes"]["value"], 0)
        self.assertEqual(by_id["pack.bytes"]["unit"], "bytes")
        self.assertEqual(by_id["pack.tokens"]["unit"], "tokens")

    def test_pack_bytes_are_canonical(self):
        code, out, err = run_measure(PACK)
        self.assertEqual(code, 0, err)
        document = json.loads(out.decode("utf-8"))
        expected = canonical(PACK.read_text(encoding="utf-8"))
        bytes_row = next(row for row in document["rows"] if row["row"] == "pack.bytes")
        self.assertEqual(bytes_row["value"], len(expected))
        self.assertEqual(bytes_row["input_sha256"], hashlib.sha256(expected).hexdigest())

    def test_output_has_no_timestamps(self):
        code, out, err = run_measure(PACK)
        self.assertEqual(code, 0, err)
        text = out.decode("utf-8")
        self.assertNotIn("generated_at", text)
        self.assertNotIn("date", text)
        self.assertIsNone(re.search(r"\d{4}-\d{2}-\d{2}T", text))

    def test_tokenizer_is_real(self):
        tokenizer = offline_tokenizer.load()
        self.assertEqual(tokenizer.count("hello world"), 2)
        self.assertGreater(tokenizer.count("antidisestablishmentarianism"), 1)

    def test_missing_package_fails_before_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            package = Path(tmp) / "tiktoken"
            package.mkdir()
            (package / "__init__.py").write_text(
                "raise ImportError('tiktoken is not installed here')\n", encoding="utf-8"
            )
            code, out, err = run_measure(PACK, env={"PYTHONPATH": str(tmp)})
        self.assertEqual(code, 2, err)
        self.assertIn("missing required tokenizer package", err)
        self.assertEqual(out, b"")

    def test_missing_asset_fails_before_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = copy_measure_tree(tmp)
            self.assertFalse((Path(tmp) / "assets").exists())
            code, out, err = _run(script, PACK)
        self.assertEqual(code, 2, err)
        self.assertIn("missing required encoding asset", err)
        self.assertEqual(out, b"")

    def test_corrupt_asset_fails_before_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = copy_measure_tree(tmp)
            assets = Path(tmp) / "assets"
            assets.mkdir()
            (assets / "cl100k_base.tiktoken").write_bytes(b"abc")
            code, out, err = _run(script, PACK)
        self.assertEqual(code, 2, err)
        self.assertIn("corrupt encoding asset", err)
        self.assertEqual(out, b"")

    def test_offline_run_succeeds(self):
        with tempfile.TemporaryDirectory() as home:
            code, out, err = run_measure(
                PACK,
                env={
                    "HOME": home,
                    "TIKTOKEN_CACHE_DIR": None,
                    "HTTP_PROXY": "http://127.0.0.1:1",
                    "HTTPS_PROXY": "http://127.0.0.1:1",
                    "ALL_PROXY": "http://127.0.0.1:1",
                    "NO_PROXY": "",
                },
            )
            self.assertEqual(code, 0, err)
            self.assertEqual(len(json.loads(out.decode("utf-8"))["rows"]), 2)
            self.assertEqual(err, "")


if __name__ == "__main__":
    unittest.main()
