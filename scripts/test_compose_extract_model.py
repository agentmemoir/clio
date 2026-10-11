#!/usr/bin/env python3
"""Tests for scripts/compose_extract_model.py.

Why these exist
---------------
`make compose up` does not go through the CLI, so the model check that protects
the `clio compose up` path does not run there. This helper is that check's second
home, and a later rename of the profile, of the overlay, or of the model file
would otherwise break the guard silently, which is the failure mode it exists to
prevent.

The engine is a stand-in `docker` that prints a resolved stack and records the
arguments it was given, so every case is decided by this machine's files and not
by what Docker is installed.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from exec_tempdir import exec_tempdir  # noqa: E402

SCRIPT = Path(__file__).resolve().parent / "compose_extract_model.py"
SH = shutil.which("sh") or "/bin/sh"
SYSTEM_PATH = os.pathsep.join(("/usr/bin", "/bin"))
DEFAULT_NAME = "numind.NuExtract-tiny-v1.5.Q4_K_M.gguf"


class ComposeExtractModelTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = exec_tempdir(purpose="the compose extract-model guard test")
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.stack_file = self.root / "stack.json"
        self.args_file = self.root / "args.txt"
        self.exit_code = 0

    def write_stub(self) -> None:
        stub = self.bin / "docker"
        stub.write_text(
            "#!/bin/sh\n"
            f"printf '%s\\n' \"$*\" >> '{self.args_file}'\n"
            f"cat '{self.stack_file}'\n"
            f"exit {self.exit_code}\n",
            encoding="utf-8",
        )
        stub.chmod(0o755)

    def engine(self, stack: dict) -> None:
        """Point the stand-in engine at the resolved stack it should print."""
        self.stack_file.write_text(json.dumps(stack), encoding="utf-8")
        self.exit_code = 0
        self.write_stub()

    def engine_refuses(self) -> None:
        """An engine that cannot resolve the stack at all."""
        self.stack_file.write_text("", encoding="utf-8")
        self.exit_code = 1
        self.write_stub()

    def extract_stack(self, name: str, source: Path) -> dict:
        return {
            "services": {
                "extract": {
                    "environment": {"LLAMA_ARG_MODEL": f"/models/{name}"},
                    "volumes": [{"type": "bind", "source": str(source), "target": "/models"}],
                }
            }
        }

    def write_model(self, name: str) -> Path:
        models = self.root / "models"
        models.mkdir(exist_ok=True)
        (models / name).write_text("stub", encoding="utf-8")
        return models

    def run_guard(
        self,
        overlay: str = "compose.arm64.yml",
        profile: str | None = "local-rft",
        path: str | None = None,
    ):
        command = [sys.executable, str(SCRIPT), "--overlay", overlay]
        if profile:
            command += ["--profile", profile]
        env = dict(os.environ)
        # The default PATH is the stand-in engine first, then the system tools.
        # A test that needs no engine passes a PATH of its own.
        env["PATH"] = path or os.pathsep.join((str(self.bin), SYSTEM_PATH))
        return subprocess.run(
            command, capture_output=True, text=True, cwd=self.root, env=env, check=False
        )

    def assert_fails_with(self, overlay: str = "compose.arm64.yml") -> str:
        result = self.run_guard(overlay)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, output)
        return output

    def test_a_present_model_passes(self) -> None:
        models = self.write_model(DEFAULT_NAME)
        self.engine(self.extract_stack(DEFAULT_NAME, models))
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_missing_model_names_the_shipped_fetch_command(self) -> None:
        self.engine(self.extract_stack(DEFAULT_NAME, self.root / "models"))
        output = self.assert_fails_with()
        self.assertIn(DEFAULT_NAME, output)
        self.assertIn("hf download", output)

    def test_a_missing_model_with_another_name_prints_no_fetch_command(self) -> None:
        self.engine(self.extract_stack("custom.gguf", self.root / "models"))
        output = self.assert_fails_with()
        self.assertIn("custom.gguf", output)
        self.assertNotIn(
            "hf download",
            output,
            "the fetch command would name a file the DevQuasar repository does not hold",
        )

    def test_a_stack_without_extract_needs_no_model(self) -> None:
        self.engine({"services": {"postgres": {}}})
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_the_engine_answer_wins_over_another_file_on_disk(self) -> None:
        # The defect this guard exists for: the file the engine names is absent
        # while a file with the shipped name is present. Checking the shipped
        # name instead of the engine's answer would start a stack that restarts
        # forever.
        models = self.write_model(DEFAULT_NAME)
        self.engine(self.extract_stack("absent.gguf", models))
        output = self.assert_fails_with()
        self.assertIn("absent.gguf", output)

    def test_an_environment_list_shape_is_read_too(self) -> None:
        models = self.write_model(DEFAULT_NAME)
        self.engine(
            {
                "services": {
                    "extract": {
                        "environment": [f"LLAMA_ARG_MODEL=/models/{DEFAULT_NAME}"],
                        "volumes": [{"type": "bind", "source": str(models), "target": "/models"}],
                    }
                }
            }
        )
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_mount_target_is_matched_as_a_whole_path_segment(self) -> None:
        # A mount at `/models` must not be read as the parent of a file under
        # `/models-other`, which would test a file the container never opens.
        models = self.root / "models"
        models.mkdir()
        (models / "numind.gguf").write_text("decoy", encoding="utf-8")
        other = self.root / "models-other"
        other.mkdir()
        (other / "numind.gguf").write_text("stub", encoding="utf-8")
        self.engine(
            {
                "services": {
                    "extract": {
                        "environment": {"LLAMA_ARG_MODEL": "/models-other/numind.gguf"},
                        "volumes": [
                            {"type": "bind", "source": str(models), "target": "/models"},
                            {"type": "bind", "source": str(other), "target": "/models-other"},
                        ],
                    }
                }
            }
        )
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        (other / "numind.gguf").unlink()
        output = self.assert_fails_with()
        self.assertIn("models-other", output)

    def test_a_missing_docker_is_reported_without_a_traceback(self) -> None:
        # The Makefile path has no Docker probe of its own, so this is the state
        # a host without Docker reaches: a message, not a stack trace. The PATH
        # is one empty directory, so no host's engine can satisfy the call.
        empty = self.root / "empty"
        empty.mkdir()
        result = self.run_guard(path=str(empty))
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 2, output)
        self.assertIn("cannot ask Docker", output)
        self.assertNotIn("Traceback", output)

    def test_a_config_call_that_fails_is_reported(self) -> None:
        self.engine_refuses()
        result = self.run_guard()
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 2, output)
        self.assertIn("docker compose config", output)

    def test_the_query_carries_the_files_and_the_profile(self) -> None:
        models = self.write_model(DEFAULT_NAME)
        self.engine(self.extract_stack(DEFAULT_NAME, models))
        self.run_guard()
        recorded = self.args_file.read_text(encoding="utf-8").strip()
        self.assertEqual(
            recorded,
            "compose -f compose.yml -f compose.arm64.yml --profile local-rft config --format json",
            "the guard must ask about the same files and profile the start uses",
        )


if __name__ == "__main__":
    unittest.main()
