"""Integration tests for the two-system baseline command (hermetic).

The baseline drives the runner through the shared registry stand-in from the
contract suite, so both real adapters are exercised without a real system. The
suite loader is patched with a synthetic suite, exactly as the runner's own
end-to-end test does, so no pinned snapshot is downloaded.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import stat
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))

from exec_tempdir import exec_tempdir  # noqa: E402

from benchmarks.clio_bench import baseline, runner  # noqa: E402
from benchmarks.tests.support import FAKE_SYSTEM, synthetic_suite  # noqa: E402


class BaselineTestCase(unittest.TestCase):
    """Shared fixtures: an executable stand-in for both registered systems."""

    def setUp(self) -> None:
        self._tmp = exec_tempdir(purpose="the baseline stand-in system")
        self.addCleanup(self._tmp.cleanup)
        self.tmpdir = self._tmp.name
        self.fake = os.path.join(self.tmpdir, "fake_system.py")
        with open(self.fake, "w", encoding="utf-8") as handle:
            handle.write(FAKE_SYSTEM)
        os.chmod(self.fake, os.stat(self.fake).st_mode | stat.S_IXUSR)
        self.state = os.path.join(self.tmpdir, "stand-in-store")
        os.makedirs(self.state)

    def run_command(self, argv: list[str]) -> int:
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
            io.StringIO()
        ):
            return baseline.main(argv)

    def argv_for(self, out: Path, md: Path, *, judge: str = "stub") -> list[str]:
        return [
            "--suite",
            "locomo",
            "--partition",
            "canary",
            "--judge",
            judge,
            "--generator",
            judge,
            "--max-probes",
            "2",
            "--max-instances",
            "1",
            "--clio-binary",
            self.fake,
            "--mnemosyne-binary",
            self.fake,
            "--clio-data-dir",
            os.path.join(self.tmpdir, "clio-data"),
            "--mnemosyne-data-dir",
            os.path.join(self.tmpdir, "mnemosyne-data"),
            "--cache-dir",
            self.tmpdir,
            "--out",
            str(out),
            "--report-md",
            str(md),
        ]


class TwoSystemBaselineTests(BaselineTestCase):
    def _run_systems(self, label: str) -> tuple[int, dict, str]:
        out = Path(self.tmpdir) / f"{label}.json"
        md = Path(self.tmpdir) / f"{label}.md"
        with mock.patch.dict(os.environ, {"CLIO_BENCH_FAKE_STATE": self.state}), mock.patch(
            "benchmarks.clio_bench.runner.load_suite", return_value=synthetic_suite()
        ):
            code = self.run_command(self.argv_for(out, md))
        payload = json.loads(out.read_text(encoding="utf-8")) if out.exists() else {}
        return code, payload, md.read_text(encoding="utf-8") if md.exists() else ""

    def test_the_report_carries_both_fingerprints_and_scores(self) -> None:
        code, payload, markdown = self._run_systems("baseline")
        self.assertEqual(code, 0)
        self.assertEqual(sorted(payload["systems"]), ["clio", "mnemosyne"])
        for name, block in payload["systems"].items():
            self.assertEqual(block["system"], name)
            self.assertTrue(block["version"])
            self.assertTrue(block["settings"])
            self.assertGreater(block["overall"]["scored_probes"], 0)
            self.assertEqual(block["overall"]["tier"], "stub")
            self.assertFalse(block["overall"]["quotable"])
            self.assertIn("locomo", block["suites"])
        self.assertIn("# clio-bench baseline (two systems)", markdown)
        self.assertIn("## clio", markdown)
        self.assertIn("## mnemosyne", markdown)
        self.assertIn("[tier stub, 2 probes; not quotable]", markdown)

    def test_the_same_command_produces_the_same_configuration_block(self) -> None:
        _, first, _ = self._run_systems("first")
        _, second, _ = self._run_systems("second")
        self.assertEqual(
            {name: block["version"] for name, block in first["systems"].items()},
            {name: block["version"] for name, block in second["systems"].items()},
        )
        self.assertEqual(
            {name: block["settings"] for name, block in first["systems"].items()},
            {name: block["settings"] for name, block in second["systems"].items()},
        )

    def test_a_broken_system_stops_the_baseline_without_a_report(self) -> None:
        out = Path(self.tmpdir) / "should-not-exist.json"
        md = Path(self.tmpdir) / "should-not-exist.md"
        argv = self.argv_for(out, md)
        argv[argv.index("--mnemosyne-binary") + 1] = "/nonexistent/mnemosyne"
        with mock.patch.dict(os.environ, {"CLIO_BENCH_FAKE_STATE": self.state}), mock.patch(
            "benchmarks.clio_bench.runner.load_suite", return_value=synthetic_suite()
        ):
            code = self.run_command(argv)
        self.assertEqual(code, 2)
        self.assertFalse(out.exists())


class BlockedGateTests(BaselineTestCase):
    def test_tier2_without_a_key_records_the_gate_and_both_fingerprints(self) -> None:
        out = Path(self.tmpdir) / "blocked.json"
        md = Path(self.tmpdir) / "blocked.md"
        with mock.patch.dict(
            os.environ,
            {"CLIO_BENCH_API_KEY": "", "OPENAI_API_KEY": "", "CLIO_BENCH_FAKE_STATE": self.state},
        ):
            code = self.run_command(self.argv_for(out, md, judge="tier2"))
        self.assertEqual(code, 2)
        payload = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(payload["status"], "blocked")
        self.assertEqual(payload["blocked_gate"]["gate"], "quotable-baseline")
        self.assertEqual(payload["blocked_gate"]["requested"]["judge"], "tier2")
        self.assertFalse(payload["blocked_gate"]["present_env"]["CLIO_BENCH_API_KEY"])
        self.assertNotIn("overall", payload)
        self.assertEqual(sorted(payload["systems"]), ["clio", "mnemosyne"])
        for block in payload["systems"].values():
            self.assertIn("settings", block)
            self.assertTrue(block["fingerprint_complete"])
        markdown = md.read_text(encoding="utf-8")
        self.assertIn("blocked gate", markdown)
        self.assertIn("tier 1 was not substituted", markdown)
        self.assertIn("## Configuration", markdown)
        # The values are rendered, not only the surfaces that returned them.
        self.assertIn("read through", markdown)
        self.assertIn('"source": "builtin"', markdown)

    def test_the_blocked_record_reads_the_pinned_data_directory(self) -> None:
        """The blocked capture and a scored run read the same directory.

        With no --data-dir flag both systems resolve the runner's own default.
        A retention store written into the clio default must therefore be the
        one the blocked record reports, not a scratch path a temp capture
        would have picked.
        """
        clio_data = os.path.join(self.tmpdir, "pinned-clio-data")
        mnemosyne_data = os.path.join(self.tmpdir, "pinned-mnemosyne-data")
        os.makedirs(clio_data)
        with open(
            os.path.join(clio_data, "retention-profiles.json"), "w", encoding="utf-8"
        ) as handle:
            handle.write('{"banks": {}, "deployment": null}')
        out = Path(self.tmpdir) / "pinned.json"
        md = Path(self.tmpdir) / "pinned.md"
        argv = self.argv_for(out, md, judge="tier2")
        for flag in ("--clio-data-dir", "--mnemosyne-data-dir"):
            index = argv.index(flag)
            del argv[index : index + 2]
        with mock.patch.dict(
            os.environ,
            {"CLIO_BENCH_API_KEY": "", "OPENAI_API_KEY": "", "CLIO_BENCH_FAKE_STATE": self.state},
        ), mock.patch.object(
            runner,
            "ADAPTER_DEFAULT_DATA_DIRS",
            {"clio": clio_data, "mnemosyne": mnemosyne_data},
        ):
            code = self.run_command(argv)
        self.assertEqual(code, 2)
        payload = json.loads(out.read_text(encoding="utf-8"))
        clio_block = payload["systems"]["clio"]
        self.assertTrue(clio_block["fingerprint_complete"])
        retention = clio_block["settings"]["retention_profile"]
        self.assertEqual(retention["value"]["source"], "bank")
        self.assertEqual(retention["value"]["bank"], "clio-bench-fingerprint")
        self.assertTrue(payload["systems"]["mnemosyne"]["fingerprint_complete"])

    def test_a_failed_fingerprint_read_is_recorded_incomplete(self) -> None:
        """A degraded capture is labelled, never shaped like a real one."""
        out = Path(self.tmpdir) / "degraded.json"
        md = Path(self.tmpdir) / "degraded.md"
        argv = self.argv_for(out, md, judge="tier2")
        argv[argv.index("--clio-binary") + 1] = "/nonexistent/clio"
        argv[argv.index("--mnemosyne-binary") + 1] = "/nonexistent/mnemosyne"
        with mock.patch.dict(
            os.environ,
            {"CLIO_BENCH_API_KEY": "", "OPENAI_API_KEY": "", "CLIO_BENCH_FAKE_STATE": self.state},
        ):
            code = self.run_command(argv)
        self.assertEqual(code, 2)
        payload = json.loads(out.read_text(encoding="utf-8"))
        clio_block = payload["systems"]["clio"]
        self.assertFalse(clio_block["fingerprint_complete"])
        self.assertEqual(
            clio_block["expected_settings"],
            ["retention_profile", "ranking_environment"],
        )
        self.assertEqual(clio_block["version"], "unreadable")
        self.assertEqual(sorted(clio_block["settings"]), ["fingerprint"])
        mnemosyne_block = payload["systems"]["mnemosyne"]
        self.assertFalse(mnemosyne_block["fingerprint_complete"])
        self.assertIn("vec_weight", mnemosyne_block["expected_settings"])
        markdown = md.read_text(encoding="utf-8")
        self.assertIn("Fingerprint incomplete", markdown)

    def test_the_blocked_record_reports_presence_and_never_the_value(self) -> None:
        planted = "sk-planted-secret"
        with mock.patch.dict(
            os.environ, {"CLIO_BENCH_API_KEY": planted, "OPENAI_API_KEY": ""}
        ):
            record = runner.blocked_gate_record(
                system_names=["clio"],
                judge="tier2",
                generator="tier2",
                detail="detail",
            )
        self.assertTrue(record["blocked_gate"]["present_env"]["CLIO_BENCH_API_KEY"])
        self.assertFalse(record["blocked_gate"]["present_env"]["OPENAI_API_KEY"])
        self.assertNotIn(planted, json.dumps(record))


class CredentialTests(BaselineTestCase):
    def test_a_planted_credential_never_reaches_the_report(self) -> None:
        planted = "sk-planted-secret-12345"
        out = Path(self.tmpdir) / "secure.json"
        md = Path(self.tmpdir) / "secure.md"
        with mock.patch.dict(
            os.environ,
            {"CLIO_BENCH_API_KEY": planted, "CLIO_BENCH_FAKE_STATE": self.state},
        ), mock.patch(
            "benchmarks.clio_bench.runner.load_suite", return_value=synthetic_suite()
        ):
            code = self.run_command(self.argv_for(out, md))
        self.assertEqual(code, 0)
        text = out.read_text(encoding="utf-8")
        self.assertNotIn(planted, text)
        self.assertNotIn("sk-", text)
        self.assertNotIn(planted, md.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
