#!/usr/bin/env python3
"""Tests for the budget check as a command: exit codes and the printed report.

The check is driven through `main` on the committed fixture pack, so the document
the build prints is the one asserted here: both pack arms, the skill truth when
no path is given, the preset targets that are reported but not enforced yet, and
every exit code. A test that measures tokens needs the pinned tokenizer and skips
with a named reason when tiktoken is absent.
"""

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import budget_check
import budget_targets

REPO_ROOT = HERE.parents[1]
PACK = REPO_ROOT / "crates" / "clio-mcp" / "tests" / "fixtures" / "schema_pack.json"

FIXTURE_DIGEST = "622890f6a700b3b5188abda7a6c1e2c196f51280703bb6babaae815d55232f3e"


def require_tokenizer():
    """Skip with a named reason when tiktoken is absent."""
    try:
        import tiktoken  # noqa: F401
    except ImportError as exc:
        raise unittest.SkipTest(f"tiktoken is not installed: {exc}") from None


def write_json(directory, name, value):
    path = Path(directory) / name
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


class CommandTests(unittest.TestCase):
    def run_main(self, argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = budget_check.main(argv)
        return code, out.getvalue()

    def test_unchanged_fixture_pack_passes_with_exit_zero(self):
        require_tokenizer()
        code, text = self.run_main(["--pack", str(PACK)])
        self.assertEqual(code, 0)
        report = json.loads(text)
        self.assertEqual(report["format"], "clio_mcp_budget_check")
        self.assertEqual(report["verdict"], "pass")
        expected_allowance = {
            "decision_id": "mcp-retention-local-defs-2026-11-06",
            "date": "2026-11-06",
            "base_commit": "3ea0d0f7ea36c120c0a62ef99e78aadae9e57a4a",
            "max_bytes": 148633,
            "max_tokens": 31667,
        }
        self.assertEqual(report["allowance"], expected_allowance)
        self.assertEqual(report["failures"], [])
        self.assertEqual(
            report["candidate"],
            {"sha256": FIXTURE_DIGEST, "bytes": 148633, "tokens": 31667},
        )
        self.assertEqual([arm["delta"] for arm in report["arms"]], [43001, 9976])
        for arm in report["arms"]:
            self.assertEqual(arm["verdict"], "pass")
            self.assertEqual(arm["allowance"], expected_allowance)

    def test_the_report_carries_the_skill_truth_and_the_targets(self):
        require_tokenizer()
        code, text = self.run_main(["--pack", str(PACK)])
        self.assertEqual(code, 0)
        report = json.loads(text)
        self.assertEqual(
            list(report),
            ["format", "format_version", "baseline", "candidate", "allowance",
             "arms", "skill", "targets", "failures", "verdict"],
        )
        self.assertEqual(report["skill"]["status"], "not-measured")
        self.assertEqual(report["skill"]["reason"], budget_check.NO_SKILL_PATH)
        self.assertEqual(
            report["skill"]["baseline_rows"],
            {
                "retired_skill_entry_point/bytes": 6393,
                "retired_skill_entry_point/tokens": 1518,
            },
        )
        targets = report["targets"]
        self.assertFalse(targets["enforced"])
        self.assertEqual(len(targets["comparisons"]), 8)
        self.assertEqual(targets["failures"], [])
        self.assertTrue(all(item["verdict"] == "miss" for item in targets["comparisons"]))

    def test_a_preset_miss_changes_the_exit_code_only_when_enforcing(self):
        require_tokenizer()
        code, text = self.run_main(["--pack", str(PACK)])
        self.assertEqual(code, 0, text)
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", True):
            code, text = self.run_main(["--pack", str(PACK)])
        self.assertEqual(code, 1, text)
        report = json.loads(text)
        self.assertEqual(report["verdict"], "fail")
        self.assertEqual(len(report["failures"]), 8, report["failures"])
        self.assertTrue(all(item["verdict"] == "miss" for item in report["targets"]["comparisons"]))

    def test_a_grown_pack_fails_with_exit_one(self):
        require_tokenizer()
        pack = json.loads(PACK.read_text(encoding="utf-8"))
        pack["tools"][0]["description"] = pack["tools"][0]["description"] + " " + "growth " * 20
        with tempfile.TemporaryDirectory() as tmp:
            path = write_json(tmp, "pack.json", pack)
            code, text = self.run_main(["--pack", str(path)])
        self.assertEqual(code, 1, text)
        report = json.loads(text)
        self.assertEqual(report["verdict"], "fail")
        self.assertTrue(report["failures"])
        self.assertTrue(any(f.startswith("bytes:") for f in report["failures"]), report)
        self.assertIsNone(report["allowance"])

    def test_an_unusable_pack_exits_two(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pack.json"
            path.write_text("not json", encoding="utf-8")
            code, text = self.run_main(["--pack", str(path)])
        self.assertEqual(code, 2)
        self.assertEqual(text, "")


if __name__ == "__main__":
    unittest.main()
