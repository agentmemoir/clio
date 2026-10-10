"""Tests for the two retention deployment profiles and their comparison.

This slice owns only this file. Every test reads the committed profiles, README,
report JSONs, comparison report, and changelog from the working tree. The live
binary test is skipped when `target/debug/clio` is not built, so the suite is
host-independent. All subprocesses carry a finite timeout.

The size test covers the hand-authored files this phase adds under
`benchmarks/`. The `profile-*.json` and `profile-*.md` run reports are generated
harness output, not authored source, so the line cap does not apply to them.
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from benchmarks.profile_diff import (  # noqa: E402
    PROFILE_SETTING,
    changed_settings,
    outside_profile,
    pair_check,
    settings_of,
)

PROFILES = ROOT / "benchmarks" / "profiles"
REPORTS = ROOT / "benchmarks" / "reports"
COMPANION_PROFILE = PROFILES / "long-lived-companion.json"
CODING_PROFILE = PROFILES / "coding-agent.json"
PROFILE_README = PROFILES / "README.md"
COMPANION_REPORT = REPORTS / "profile-long-lived-companion.json"
CODING_REPORT = REPORTS / "profile-coding-agent.json"
COMPARISON = REPORTS / "two-profiles-comparison.md"
CHANGELOG = ROOT / "CHANGELOG.md"
RUN_PY = ROOT / "benchmarks" / "run.py"
BINARY = ROOT / "target" / "debug" / "clio"

REQUIRED_KEYS = frozenset(
    {
        "verbosity",
        "theta_offsets",
        "duplicate_tolerance_write",
        "duplicate_tolerance_read",
        "recall_scope_default",
        "retention_curve",
        "retention_floor",
        "pack_fidelity_default",
        "provenance_floor_default",
        "maintenance_trigger",
        "version",
    }
)

# (companion value, coding value) for the six keys that must differ.
DIFFERING = {
    "verbosity": ("balanced", "selective"),
    "duplicate_tolerance_write": ("lenient", "strict"),
    "duplicate_tolerance_read": ("lenient", "strict"),
    "recall_scope_default": ("full", "consolidated_only"),
    "retention_floor": (0.5, 0.1),
    "provenance_floor_default": (None, "verified"),
}

# The five keys on which the two profiles must agree.
SAME = {
    "theta_offsets": {},
    "retention_curve": True,
    "pack_fidelity_default": "flow",
    "maintenance_trigger": True,
    "version": 3,
}

# The hand-authored files this phase adds under benchmarks/. Generated run
# reports (benchmarks/reports/profile-*.json and .md) are excluded.
PHASE_AUTHORED_FILES = (
    ROOT / "benchmarks" / "profile_diff.py",
    COMPANION_PROFILE,
    CODING_PROFILE,
    PROFILE_README,
    COMPARISON,
    REPORTS / "two-profiles-run-notes.md",
    Path(__file__).resolve(),
)


def _load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _with_timeout(argv: list[str]) -> list[str]:
    """Prefix `timeout 60` when coreutils `timeout` is available."""
    if shutil.which("timeout"):
        return ["timeout", "60", *argv]
    return argv


def _section(text: str, heading: str) -> str:
    """Return the text from `heading` up to the next heading of its level."""
    start = text.index(heading)
    level = len(heading) - len(heading.lstrip("#"))
    rest = text[start + len(heading) :]
    match = re.search(rf"^#{{1,{level}}} ", rest, re.MULTILINE)
    return rest[: match.start()] if match else rest


class ProfileFileTests(unittest.TestCase):
    def test_each_profile_file_has_the_eleven_required_keys(self) -> None:
        for path in (COMPANION_PROFILE, CODING_PROFILE):
            with self.subTest(profile=path.name):
                profile = _load_json(path)
                self.assertTrue(REQUIRED_KEYS <= set(profile))
                self.assertTrue(set(profile) <= REQUIRED_KEYS | {"mission"})
                self.assertIsNone(profile.get("mission"))

    @unittest.skipUnless(BINARY.exists(), "target/debug/clio is not built")
    def test_each_profile_file_loads_through_the_retention_set_command(self) -> None:
        for profile in (COMPANION_PROFILE, CODING_PROFILE):
            with (
                self.subTest(profile=profile.name),
                tempfile.TemporaryDirectory() as tmp,
            ):
                store = Path(tmp) / "store.json"
                argv = [
                    str(BINARY),
                    "retention",
                    "set",
                    "--profiles",
                    str(store),
                    "--scope",
                    "deployment",
                    "--profile-file",
                    str(profile),
                ]
                result = subprocess.run(
                    _with_timeout(argv),
                    capture_output=True,
                    text=True,
                    timeout=90,
                    cwd=ROOT,
                )
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_the_two_profiles_differ_in_exactly_six_keys(self) -> None:
        companion = _load_json(COMPANION_PROFILE)
        coding = _load_json(CODING_PROFILE)
        changed = changed_settings(companion, coding)
        self.assertEqual(set(changed), set(DIFFERING))
        for key, (companion_value, coding_value) in DIFFERING.items():
            with self.subTest(key=key):
                self.assertEqual(
                    changed[key], {"a": companion_value, "b": coding_value}
                )
                self.assertEqual(companion[key], companion_value)
                self.assertEqual(coding[key], coding_value)
        for key, value in SAME.items():
            with self.subTest(key=key):
                self.assertEqual(companion[key], value)
                self.assertEqual(coding[key], value)


class ReadmeTests(unittest.TestCase):
    def test_the_readme_one_sentence_difference_names_all_six_fields(self) -> None:
        text = PROFILE_README.read_text(encoding="utf-8")
        self.assertIn("unmeasured starting point", text)
        section = _section(text, "## The one-sentence difference")
        sentence = next(line for line in section.splitlines() if line.strip())
        for key in DIFFERING:
            self.assertIn(key, sentence, key)
        for companion_value, coding_value in DIFFERING.values():
            for value in (companion_value, coding_value):
                self.assertIn("null" if value is None else str(value), sentence)


class FingerprintDiffTests(unittest.TestCase):
    def test_the_committed_report_diff_is_profile_only(self) -> None:
        companion = settings_of(_load_json(COMPANION_REPORT))
        coding = settings_of(_load_json(CODING_REPORT))
        changed = changed_settings(companion, coding)
        self.assertEqual(list(changed), [PROFILE_SETTING])
        self.assertEqual(outside_profile(changed), [])
        companion_value = changed[PROFILE_SETTING]["a"]["value"]
        coding_value = changed[PROFILE_SETTING]["b"]["value"]
        self.assertEqual(companion_value["verbosity"], "balanced")
        self.assertEqual(coding_value["verbosity"], "selective")
        self.assertEqual(companion_value["duplicate_tolerance_write"], "lenient")
        self.assertEqual(coding_value["duplicate_tolerance_write"], "strict")
        self.assertEqual(
            companion_value["accepted_but_inert"]["duplicate_tolerance_read"],
            "lenient",
        )
        self.assertEqual(
            coding_value["accepted_but_inert"]["duplicate_tolerance_read"],
            "strict",
        )
        self.assertEqual(
            companion_value["accepted_but_inert"]["recall_scope_default"], "full"
        )
        self.assertEqual(
            coding_value["accepted_but_inert"]["recall_scope_default"],
            "consolidated_only",
        )

    def test_a_fabricated_outside_change_is_flagged(self) -> None:
        a = {"ranking_environment": {"status": "read", "value": 1}}
        b = {"ranking_environment": {"status": "read", "value": 2}}
        changed = changed_settings(a, b)
        self.assertEqual(outside_profile(changed), ["ranking_environment"])
        self.assertEqual(changed_settings(a, a), {})

    def test_identical_fingerprints_show_no_change(self) -> None:
        companion = settings_of(_load_json(COMPANION_REPORT))
        coding = settings_of(_load_json(CODING_REPORT))
        self.assertEqual(changed_settings(companion, companion), {})
        self.assertTrue(changed_settings(companion, coding))
        ok, reason = pair_check(changed_settings(companion, companion))
        self.assertFalse(ok, reason)
        ok, reason = pair_check({"verbosity": {"a": 1, "b": 2}})
        self.assertFalse(ok, reason)
        ok, reason = pair_check(changed_settings(companion, coding))
        self.assertTrue(ok, reason)


class ReportNumberTests(unittest.TestCase):
    def test_every_score_line_carries_a_tier_and_probe_count(self) -> None:
        text = COMPARISON.read_text(encoding="utf-8")
        checked = 0
        for line in text.splitlines():
            if line.startswith("#"):
                continue
            if not re.search(r"\b(accuracy|f1)\b", line, re.IGNORECASE):
                continue
            if not re.search(r"\d", line):
                continue
            checked += 1
            lowered = line.lower()
            self.assertIn("tier", lowered, line)
            self.assertIn("probe", lowered, line)
            self.assertRegex(lowered, r"quotab|quote", line)
        self.assertGreater(checked, 0, "no score line was found to check")

    def test_every_aggregate_block_is_a_stub_smoke_signal(self) -> None:
        for path in (COMPANION_REPORT, CODING_REPORT):
            report = _load_json(path)
            for suite_name, suite in report["suites"].items():
                with self.subTest(report=path.name, suite=suite_name):
                    aggregate = suite["aggregate"]
                    self.assertFalse(aggregate["quotable"])
                    self.assertEqual(aggregate["tier"], "stub")
        text = COMPARISON.read_text(encoding="utf-8")
        self.assertIn("No stub number may be presented as a result", text)


class HarnessTests(unittest.TestCase):
    def test_the_harness_has_no_profile_or_config_flag(self) -> None:
        result = subprocess.run(
            _with_timeout([sys.executable, str(RUN_PY), "--help"]),
            capture_output=True,
            text=True,
            timeout=90,
            cwd=ROOT,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("--profile", result.stdout)
        self.assertNotIn("--config", result.stdout)
        self.assertIn("--suite", result.stdout)
        self.assertIn("--judge", result.stdout)


class ConstantTests(unittest.TestCase):
    def test_each_listed_constant_is_an_unmeasured_starting_point(self) -> None:
        text = COMPARISON.read_text(encoding="utf-8")
        self.assertIn("unmeasured starting point", text)
        for name in (
            "retention.half_life_days",
            "retention_floor",
            "pack_fidelity_default",
        ):
            self.assertIn(name, text)

    def test_the_decision_rule_points_at_supersession_not_a_weight(self) -> None:
        section = _section(
            COMPARISON.read_text(encoding="utf-8"), "## Decision rule"
        )
        self.assertIn("knowledge-update", section)
        self.assertIn("supersession", section)
        self.assertIn("not at a weight", section)


class ReportBaselineTests(unittest.TestCase):
    def test_the_report_states_there_is_no_quotable_baseline(self) -> None:
        text = COMPARISON.read_text(encoding="utf-8").lower()
        self.assertIn("no quotable baseline", text)

    def test_the_report_names_both_bank_counts(self) -> None:
        text = COMPARISON.read_text(encoding="utf-8")
        self.assertIn("509", text)
        self.assertIn("508", text)


class ChangelogTests(unittest.TestCase):
    def test_the_changelog_entry_names_both_profiles_and_no_constant_change(
        self,
    ) -> None:
        text = CHANGELOG.read_text(encoding="utf-8")
        # One entry per top-level bullet, with its wrapped lines joined, so the
        # check does not depend on where the prose wraps or on a heading name.
        entries = [
            entry
            for entry in re.split(r"\n(?=- )", text)
            if "long-lived-companion.json" in entry and "coding-agent.json" in entry
        ]
        self.assertTrue(entries, "no changelog entry names both profiles")
        entry = entries[0]
        self.assertIn("no constant", entry.lower())
        self.assertNotIn("accuracy", entry)


class SizeTests(unittest.TestCase):
    def test_each_authored_phase_file_stays_within_the_line_cap(self) -> None:
        for path in PHASE_AUTHORED_FILES:
            with self.subTest(path=path.name):
                self.assertTrue(path.exists(), path)
                lines = len(path.read_bytes().splitlines())
                self.assertLessEqual(lines, 450, f"{path} has {lines} lines")


if __name__ == "__main__":
    unittest.main()
