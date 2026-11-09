#!/usr/bin/env python3
"""Tests for the budget check as a command: exit codes and the printed report.

The check is driven through `main` on the committed fixture pack, so the document
the build prints is the one asserted here: both pack arms, the skill truth when
no path is given, the activated body that is always measured, the per-preset
targets that fail the build on a miss, and every exit code. A test that
measures tokens needs the pinned tokenizer and skips with a named reason when
tiktoken is absent.
"""

import contextlib
import hashlib
import io
import json
import os
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

FIXTURE_DIGEST = "f9820131ad2b5f698e08280d0cf5f6226c2aa5e35258a27568d4fb172f592764"

ACTIVATED_ROW = "activated_skill_body/tokens"
ACTIVATED_TOKENS = 594
BODY = "Clio MCP activated skill body.\n" * 2


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

    def passing(self):
        """Return one at-target measurement per preset, so the target arm passes."""
        return {
            preset: {
                "sha256": "ab" * 32,
                "bytes": 140559 if preset == "all" else 84505,
                "tokens": 30004 if preset == "all" else 15183,
            }
            for preset in budget_targets.TARGET_PRESETS
        }

    def run_live(self, argv):
        measured = self.passing()
        with mock.patch.object(
            budget_check.preset_rows, "measure_presets", return_value=measured
        ):
            return self.run_main(argv)

    def test_unchanged_fixture_pack_passes_with_exit_zero(self):
        require_tokenizer()
        code, text = self.run_live(["--pack", str(PACK), "--preset-bin", "clio"])
        self.assertEqual(code, 0)
        report = json.loads(text)
        self.assertEqual(report["format"], "clio_mcp_budget_check")
        self.assertEqual(report["verdict"], "pass")
        expected_allowance = {
            "decision_id": "mcp-flattened-pack-shape-2026-11-09",
            "date": "2026-11-09",
            "base_commit": "2586d7f35e6d7cb35dbe321125f4731e55ad1ab5",
            "max_bytes": 140559,
            "max_tokens": 30004,
        }
        self.assertEqual(report["allowance"], expected_allowance)
        self.assertEqual(report["failures"], [])
        self.assertEqual(
            report["candidate"],
            {"sha256": FIXTURE_DIGEST, "bytes": 140559, "tokens": 30004},
        )
        self.assertEqual([arm["delta"] for arm in report["arms"]], [34927, 8313])
        for arm in report["arms"]:
            self.assertEqual(arm["verdict"], "pass")
            self.assertEqual(arm["allowance"], expected_allowance)

    def test_the_report_carries_the_skill_truth_and_the_targets(self):
        require_tokenizer()
        code, text = self.run_live(["--pack", str(PACK), "--preset-bin", "clio"])
        self.assertEqual(code, 0)
        report = json.loads(text)
        self.assertEqual(
            list(report),
            ["format", "format_version", "baseline", "candidate", "allowance",
             "arms", "skill", "activated", "targets", "descriptions", "failures",
             "verdict"],
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
        self.assertTrue(targets["enforced"])
        self.assertEqual(len(targets["comparisons"]), 8)
        self.assertEqual(
            {item["preset"] for item in targets["comparisons"]},
            set(budget_targets.TARGET_PRESETS),
        )
        self.assertEqual(targets["failures"], [])
        all_rows = [
            item for item in targets["comparisons"] if item["preset"] == "all"
        ]
        self.assertTrue(all(item["verdict"] == "pass" for item in all_rows))

    def test_a_preset_miss_changes_the_exit_code_only_when_enforcing(self):
        require_tokenizer()
        over = {
            "sha256": "ab" * 32,
            "bytes": 90000,
            "tokens": 20000,
        }
        measured = {preset: dict(over) for preset in budget_targets.TARGET_PRESETS}
        with mock.patch.object(
            budget_check.preset_rows, "measure_presets", return_value=measured
        ):
            with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", False):
                code, text = self.run_main(
                    ["--pack", str(PACK), "--preset-bin", "clio"]
                )
            self.assertEqual(code, 0, text)
            report = json.loads(text)
            self.assertEqual(report["verdict"], "pass")
            self.assertEqual(report["targets"]["failures"], [])
            self.assertTrue(
                any(
                    item["verdict"] == "miss"
                    for item in report["targets"]["comparisons"]
                )
            )
            with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", True):
                code, text = self.run_main(
                    ["--pack", str(PACK), "--preset-bin", "clio"]
                )
        self.assertEqual(code, 1, text)
        report = json.loads(text)
        self.assertEqual(report["verdict"], "fail")
        ops_tokens = next(
            failure
            for failure in report["failures"]
            if "'ops'" in failure and "tokens" in failure
        )
        for expected in ("'ops'", "tokens", "15183", "20000", "4817", "-4817"):
            self.assertIn(expected, ops_tokens)

    def test_every_preset_is_measured(self):
        require_tokenizer()
        seen = {}

        def fake(binary, tokenizer, presets):
            seen["binary"] = binary
            seen["presets"] = tuple(presets)
            return {
                preset: {"sha256": "ab" * 32, "bytes": 1, "tokens": 1}
                for preset in presets
            }

        with mock.patch.object(budget_check.preset_rows, "measure_presets", side_effect=fake):
            code, text = self.run_main(["--pack", str(PACK), "--preset-bin", "clio"])
        self.assertEqual(code, 0, text)
        self.assertEqual(seen["binary"], "clio")
        self.assertEqual(seen["presets"], budget_targets.TARGET_PRESETS)

    def test_a_missing_preset_measurement_exits_two(self):
        require_tokenizer()
        measured = {
            preset: {"sha256": "ab" * 32, "bytes": 1, "tokens": 1}
            for preset in budget_targets.TARGET_PRESETS
            if preset != "ops"
        }
        err = io.StringIO()
        with mock.patch.object(
            budget_check.preset_rows, "measure_presets", return_value=measured
        ):
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
                code = budget_check.main(["--pack", str(PACK), "--preset-bin", "clio"])
        self.assertEqual(code, 2)
        self.assertIn("targets: no measured candidate for preset 'ops'", err.getvalue())

    def test_the_flip_passes_on_committed_presets(self):
        require_tokenizer()
        try:
            binary = budget_check.preset_rows._binary(None)
        except budget_check.BudgetError as exc:
            self.skipTest(f"no usable clio binary: {exc}")
        with mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", True):
            code, text = self.run_main(["--pack", str(PACK), "--preset-bin", str(binary)])
        self.assertEqual(code, 0, text)
        report = json.loads(text)
        self.assertEqual(report["targets"]["failures"], [])
        self.assertTrue(
            all(item["verdict"] == "pass" for item in report["targets"]["comparisons"])
        )

    def test_target_failures_are_separate_from_growth_failures(self):
        require_tokenizer()
        measured = {
            preset: {"sha256": "ab" * 32, "bytes": 90000, "tokens": 20000}
            for preset in budget_targets.TARGET_PRESETS
        }
        with mock.patch.object(
            budget_check.preset_rows, "measure_presets", return_value=measured
        ), mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", False):
            code, text = self.run_main(["--pack", str(PACK), "--preset-bin", "clio"])
        self.assertEqual(code, 0, text)
        report = json.loads(text)
        self.assertEqual(report["targets"]["failures"], [])
        self.assertEqual(report["failures"], [])
        with mock.patch.object(
            budget_check.preset_rows, "measure_presets", return_value=measured
        ), mock.patch.object(budget_targets, "PRESET_TARGETS_ENFORCED", True):
            code, text = self.run_main(["--pack", str(PACK), "--preset-bin", "clio"])
        self.assertEqual(code, 1, text)
        report = json.loads(text)
        self.assertTrue(report["targets"]["failures"])
        self.assertFalse(
            any(
                failure.startswith(("bytes:", "tokens:"))
                for failure in report["failures"]
            )
        )
        self.assertTrue(all(arm["verdict"] == "pass" for arm in report["arms"]))

    def test_a_grown_pack_fails_with_exit_one(self):
        require_tokenizer()
        pack = json.loads(PACK.read_text(encoding="utf-8"))
        pack["tools"][0]["description"] = pack["tools"][0]["description"] + " " + "growth " * 20
        with tempfile.TemporaryDirectory() as tmp:
            path = write_json(tmp, "pack.json", pack)
            code, text = self.run_live(["--pack", str(path), "--preset-bin", "clio"])
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
            code, text = self.run_main(["--pack", str(path), "--preset-bin", "clio"])
        self.assertEqual(code, 2)
        self.assertEqual(text, "")

    def test_targets_refuse_without_a_preset_binary(self):
        require_tokenizer()
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            code = budget_check.main(["--pack", str(PACK)])
        self.assertEqual(code, 2)
        message = err.getvalue()
        self.assertIn("targets:", message)
        self.assertIn("--preset-bin", message)


class ActivatedCommandTests(unittest.TestCase):
    """The activated body is measured on every run, against its frozen row."""

    def run_main(self, argv):
        if "--preset-bin" not in argv:
            argv = [*argv, "--preset-bin", "clio"]
        measured = {
            preset: {
                "sha256": "ab" * 32,
                "bytes": 140559 if preset == "all" else 84505,
                "tokens": 30004 if preset == "all" else 15183,
            }
            for preset in budget_targets.TARGET_PRESETS
        }
        out = io.StringIO()
        err = io.StringIO()
        with mock.patch.object(
            budget_check.preset_rows, "measure_presets", return_value=measured
        ), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = budget_check.main(argv)
        return code, out.getvalue(), err.getvalue()

    def measured(self, text):
        return json.loads(text)["activated"]

    def test_the_shipped_body_is_measured_with_no_flag(self):
        require_tokenizer()
        code, text, err = self.run_main(["--pack", str(PACK)])
        self.assertEqual(code, 0, text + err)
        activated = self.measured(text)
        self.assertEqual(activated["status"], "measured")
        self.assertEqual(activated["row"], ACTIVATED_ROW)
        self.assertEqual(activated["file"], "skills/clio-mcp/SKILL.md")
        self.assertEqual(activated["tokens"], ACTIVATED_TOKENS)
        self.assertEqual(activated["ceiling"], ACTIVATED_TOKENS)
        self.assertEqual(activated["failures"], [])
        self.assertTrue(activated["digest_matches"])
        self.assertEqual(json.loads(text)["failures"], [])

    def test_the_shipped_body_is_measured_from_another_directory(self):
        require_tokenizer()
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as tmp:
            os.chdir(tmp)
            try:
                code, text, err = self.run_main(["--pack", str(PACK)])
            finally:
                os.chdir(previous)
        self.assertEqual(code, 0, text + err)
        self.assertEqual(self.measured(text)["tokens"], ACTIVATED_TOKENS)

    def test_an_activated_skill_path_overrides_the_shipped_body(self):
        require_tokenizer()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "body.md"
            path.write_text(BODY, encoding="utf-8")
            expected = hashlib.sha256(path.read_bytes()).hexdigest()
            code, text, err = self.run_main(
                ["--pack", str(PACK), "--activated-skill", str(path)]
            )
        self.assertEqual(code, 0, text + err)
        activated = self.measured(text)
        self.assertEqual(activated["sha256"], expected)
        self.assertEqual(activated["bytes"], len(BODY.encode("utf-8")))
        self.assertLess(activated["tokens"], ACTIVATED_TOKENS)
        self.assertFalse(activated["digest_matches"])

    def test_activated_body_growth_exits_one(self):
        require_tokenizer()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "body.md"
            path.write_text("a " * 1600, encoding="utf-8")
            code, text, err = self.run_main(
                ["--pack", str(PACK), "--activated-skill", str(path)]
            )
        self.assertEqual(code, 1, text + err)
        report = json.loads(text)
        self.assertEqual(report["verdict"], "fail")
        activated = report["activated"]
        self.assertEqual(report["failures"], activated["failures"])
        self.assertEqual(
            activated["failures"],
            [
                f"{ACTIVATED_ROW}: candidate {activated['tokens']} exceeds baseline "
                f"{ACTIVATED_TOKENS} by {activated['tokens'] - ACTIVATED_TOKENS}"
            ],
        )

    def test_an_explicitly_empty_activated_skill_path_exits_two(self):
        code, text, err = self.run_main(["--pack", str(PACK), "--activated-skill", ""])
        self.assertEqual(code, 2, text + err)
        self.assertEqual(text, "")
        self.assertEqual(err, "mcp-budget-check: --activated-skill path is empty\n")


if __name__ == "__main__":
    unittest.main()
