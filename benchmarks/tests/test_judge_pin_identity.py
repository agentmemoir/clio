"""Judge identity pins: the artifact revision and the served name are checkable."""

from __future__ import annotations

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from benchmarks.clio_bench.cli import _judge_settings, build_parser, main
from benchmarks.clio_bench.fetch import load_pins
from benchmarks.clio_bench.runner import _DEFAULT_PINS

TIER1_REVISION = (
    "Qwen/Qwen2.5-1.5B-Instruct-GGUF@91cad51170dc346986eccefdc2dd33a9da36ead9"
)


class JudgePinIdentityTests(unittest.TestCase):
    def test_a_tier1_pin_resolves_its_real_artifact_revision(self) -> None:
        args = build_parser().parse_args(["--judge", "local"])
        pins = load_pins(_DEFAULT_PINS)
        tier, model, served_model, revision, base_url = _judge_settings(args, pins)
        self.assertEqual(tier, "tier1")
        self.assertEqual(model, "qwen2.5-1.5b-instruct")
        self.assertEqual(served_model, "qwen2.5-1.5b-instruct")
        self.assertEqual(revision, TIER1_REVISION)
        self.assertEqual(base_url, "http://127.0.0.1:8899")

    def test_a_diverging_judge_model_override_exits_two_without_a_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            md = Path(tmp) / "should-not-exist.md"
            stderr = io.StringIO()
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite"
            ) as load, contextlib.redirect_stdout(
                io.StringIO()
            ), contextlib.redirect_stderr(stderr):
                code = main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "local",
                        "--judge-model",
                        "other-model",
                        "--generator",
                        "stub",
                        "--out",
                        str(out),
                        "--report-md",
                        str(md),
                        "--cache-dir",
                        tmp,
                    ]
                )
                load.assert_not_called()
            self.assertEqual(code, 2)
            self.assertFalse(out.exists())
            self.assertFalse(md.exists())
            message = stderr.getvalue()
            self.assertIn("other-model", message)
            self.assertIn("qwen2.5-1.5b-instruct", message)
            self.assertIn(str(_DEFAULT_PINS), message)

    def test_a_judge_pin_without_a_served_model_produces_no_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            md = Path(tmp) / "should-not-exist.md"
            pins = load_pins(_DEFAULT_PINS)
            del pins["judge"]["tier1"]["served_model"]
            pins_path = Path(tmp) / "pins.json"
            pins_path.write_text(json.dumps(pins), encoding="utf-8")
            stderr = io.StringIO()
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite"
            ) as load, contextlib.redirect_stdout(
                io.StringIO()
            ), contextlib.redirect_stderr(stderr):
                code = main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "local",
                        "--generator",
                        "stub",
                        "--pins",
                        str(pins_path),
                        "--out",
                        str(out),
                        "--report-md",
                        str(md),
                        "--cache-dir",
                        tmp,
                    ]
                )
                load.assert_not_called()
            self.assertEqual(code, 2)
            self.assertFalse(out.exists())
            self.assertFalse(md.exists())
            message = stderr.getvalue()
            self.assertIn("served_model", message)
            self.assertIn("tier1", message)

    def test_a_generator_pin_without_a_revision_produces_no_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "should-not-exist.json"
            pins = load_pins(_DEFAULT_PINS)
            del pins["judge"]["tier1"]["revision"]
            pins_path = Path(tmp) / "pins.json"
            pins_path.write_text(json.dumps(pins), encoding="utf-8")
            stderr = io.StringIO()
            with mock.patch(
                "benchmarks.clio_bench.cli.load_suite"
            ) as load, contextlib.redirect_stdout(
                io.StringIO()
            ), contextlib.redirect_stderr(stderr):
                code = main(
                    [
                        "--suite",
                        "locomo",
                        "--adapter",
                        "memory",
                        "--judge",
                        "stub",
                        "--generator",
                        "local",
                        "--pins",
                        str(pins_path),
                        "--out",
                        str(out),
                        "--cache-dir",
                        tmp,
                    ]
                )
                load.assert_not_called()
            self.assertEqual(code, 2)
            self.assertFalse(out.exists())
            message = stderr.getvalue()
            self.assertIn("generator pin invalid", message)
            self.assertIn("revision", message)


if __name__ == "__main__":
    unittest.main()
