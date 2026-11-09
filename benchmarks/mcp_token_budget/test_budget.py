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
REPO_ROOT = HERE.parents[1]
SCRIPT = REPO_ROOT / "scripts" / "mcp_token_budget.sh"
PACK = REPO_ROOT / "crates" / "clio-mcp" / "tests" / "fixtures" / "schema_pack.json"

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

    def test_pretty_and_compact_packs_produce_identical_output(self):
        """Input formatting must not move a row: the canonical form defines the measurement.

        The committed fixture is pretty-printed, so its on-disk bytes are not the
        bytes that get measured. Feeding `measure.py` that fixture and feeding it
        the same pack re-serialized compactly (plus the trailing newline the CLI
        writes) must produce byte-identical stdout, because the script parses and
        re-serializes the whole pack before measuring anything.
        """
        pretty_bytes = PACK.read_bytes()
        pack_object = json.loads(pretty_bytes.decode("utf-8"))
        compact_bytes = (
            json.dumps(
                pack_object,
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            ).encode("utf-8")
            + b"\n"
        )
        self.assertNotEqual(
            pretty_bytes, compact_bytes, "fixture is already canonical; test proves nothing"
        )

        pretty_exit, pretty_out, pretty_err = run_measure(PACK)
        self.assertEqual(pretty_exit, 0, pretty_err)

        handle, compact_name = tempfile.mkstemp(
            prefix="clio-compact-pack-", suffix=".json"
        )
        compact_path = Path(compact_name)
        try:
            with os.fdopen(handle, "wb") as stream:
                stream.write(compact_bytes)
            compact_exit, compact_out, compact_err = run_measure(compact_path)
        finally:
            os.unlink(compact_path)
        self.assertEqual(compact_exit, 0, compact_err)
        self.assertFalse(compact_path.exists())
        self.assertEqual(pretty_out, compact_out)

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

    def test_pack_tokens_match_direct_tiktoken(self):
        """Tie the emitted `pack.tokens` row to a real cl100k_base token count.

        The count is computed here straight from the committed encoding asset, so
        it never goes through `offline_tokenizer.load()`: a tokenizer that only
        estimates (bytes / 4, word count, ...) cannot satisfy this test.
        """
        import tiktoken

        code, out, err = run_measure(PACK)
        self.assertEqual(code, 0, err)
        rows = {row["row"]: row for row in json.loads(out.decode("utf-8"))["rows"]}

        # Same canonicalization as measure.py: sorted keys, no whitespace, UTF-8.
        pack = json.loads(PACK.read_text(encoding="utf-8"))
        canonical_bytes = json.dumps(
            pack, ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
        canonical_text = canonical_bytes.decode("utf-8")

        # Seed a private tiktoken cache with the repo asset under its blob URL hash,
        # so get_encoding reads local bytes and never touches the network.
        cache_key = hashlib.sha1(
            b"https://openaipublic.blob.core.windows.net/encodings/cl100k_base.tiktoken"
        ).hexdigest()
        cache_dir = tempfile.mkdtemp(prefix="clio-tiktoken-")
        previous_cache_dir = os.environ.get("TIKTOKEN_CACHE_DIR")
        try:
            shutil.copyfile(
                HERE / "assets" / "cl100k_base.tiktoken",
                Path(cache_dir) / cache_key,
            )
            os.environ["TIKTOKEN_CACHE_DIR"] = cache_dir
            encoding = tiktoken.get_encoding("cl100k_base")
            expected = len(encoding.encode_ordinary(canonical_text))
        finally:
            if previous_cache_dir is None:
                os.environ.pop("TIKTOKEN_CACHE_DIR", None)
            else:
                os.environ["TIKTOKEN_CACHE_DIR"] = previous_cache_dir
            shutil.rmtree(cache_dir, ignore_errors=True)

        self.assertEqual(rows["pack.bytes"]["value"], len(canonical_bytes), rows)
        self.assertEqual(rows["pack.tokens"]["value"], expected, rows)
        # The committed fixture is a stable snapshot: a schema change must update this pin deliberately.
        self.assertEqual(expected, 30081, "fixture token count moved; re-pin after verifying")
        # Estimators must fail: neither a bytes/4 constant nor a word count may pass.
        self.assertNotEqual(rows["pack.tokens"]["value"], len(canonical_bytes) // 4)
        self.assertNotEqual(rows["pack.tokens"]["value"], len(canonical_text.split()))

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

    def test_cache_miss_is_refused_instead_of_downloading(self):
        """A wrong cache key must raise, never fall through to an HTTPS fetch.

        The check runs in a fresh interpreter because tiktoken caches loaded
        encodings in-process: a warm encoding would hide the bug entirely.
        """
        dead_proxy = {
            "HTTP_PROXY": "http://127.0.0.1:1",
            "HTTPS_PROXY": "http://127.0.0.1:1",
            "ALL_PROXY": "http://127.0.0.1:1",
            "NO_PROXY": "",
        }
        script = (
            "import sys\n"
            f"sys.path.insert(0, {str(HERE)!r})\n"
            "import offline_tokenizer\n"
            "offline_tokenizer._CACHE_KEY = '0' * 40\n"
            "try:\n"
            "    offline_tokenizer.load()\n"
            "except offline_tokenizer.BudgetError as exc:\n"
            "    sys.exit(0 if 'network access attempted at measurement time' in str(exc) else 1)\n"
            "sys.exit(1)\n"
        )
        for label, env in (("normal env", None), ("dead proxy", dead_proxy)):
            with self.subTest(env=label):
                child_env = dict(os.environ)
                for key, value in (env or {}).items():
                    child_env[key] = value
                completed = subprocess.run(
                    [sys.executable, "-c", script],
                    capture_output=True,
                    env=child_env,
                    cwd=str(HERE),
                    check=False,
                )
                self.assertEqual(
                    completed.returncode,
                    0,
                    "expected a BudgetError, got exit %d: stdout=%r stderr=%r"
                    % (
                        completed.returncode,
                        completed.stdout.decode("utf-8", "replace"),
                        completed.stderr.decode("utf-8", "replace"),
                    ),
                )


class DocumentedCommandTests(unittest.TestCase):
    """The documented `scripts/mcp_token_budget.sh` wrapper, driven end to end."""

    def test_script_reports_export_failure_cleanly(self):
        """A failed `mcp schema-export` must fail the command and emit no rows.

        The wrapper exports into a temp file and checks that status before it runs
        the measurement, so this path never needs tiktoken: the export fails first.
        """
        self.assertTrue(SCRIPT.is_file(), f"missing wrapper script: {SCRIPT}")
        child_env = dict(os.environ)
        child_env["CLIO_BIN"] = "/bin/false"
        child_env["PYTHON"] = sys.executable
        completed = subprocess.run(
            [str(SCRIPT)],
            cwd=str(REPO_ROOT),
            capture_output=True,
            env=child_env,
            check=False,
        )
        stderr = completed.stderr.decode("utf-8", "replace")
        self.assertEqual(completed.returncode, 1, stderr)
        self.assertEqual(completed.stdout, b"", stderr)
        self.assertIn("clio mcp schema-export failed", stderr)


if __name__ == "__main__":
    unittest.main()
