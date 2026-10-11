#!/usr/bin/env python3
"""Refuse to start a Compose stack whose extract service has no model on the host.

Why this exists
---------------
`make compose up` runs `docker compose` itself, so the check inside the CLI never
runs on that path. On a host with no GGUF the extract container started, failed
to open its model, and was restarted forever, while the start command reported
success.

What it does
------------
The name of the model is a Compose variable, so three things can set it: the
process environment, the project `.env`, and the default inside the compose file.
Compose resolves them in that order, and the environment wins over `.env`. This
asks Compose for the resolved stack rather than repeating that order and the
`.env` grammar, so the file it refuses to start without is the file the container
would actually open.

The CLI asks Compose the same question in Rust (see `compose_model.rs`). The rule
is stated once and read twice because the two paths run in different languages;
the authority is the engine in both cases.

    python3 scripts/compose_extract_model.py --overlay compose.arm64.yml --profile local-rft

Exit codes: 0 the stack needs no model, or the model is present; 1 the model is
missing; 2 Compose could not be asked.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

MODEL_ARG = "LLAMA_ARG_MODEL"
DEFAULT_NAME = "numind.NuExtract-tiny-v1.5.Q4_K_M.gguf"
FETCH_COMMAND = (
    "hf download DevQuasar/numind.NuExtract-tiny-v1.5-GGUF "
    f"{DEFAULT_NAME} --local-dir ./models"
)


class ConfigUnavailable(Exception):
    """Compose was asked and did not answer with a resolved stack."""


def compose_command(overlay: str, profile: str | None) -> list[str]:
    """The resolved-stack query, carrying the same files and profile as the start."""
    command = ["docker", "compose", "-f", "compose.yml", "-f", overlay]
    if profile:
        command += ["--profile", profile]
    return command + ["config", "--format", "json"]


def resolved_stack(overlay: str, profile: str | None) -> dict:
    """The stack Compose resolves for this selection."""
    try:
        result = subprocess.run(
            compose_command(overlay, profile), capture_output=True, text=True, check=False
        )
    except OSError as error:
        raise ConfigUnavailable(f"cannot ask Docker for the resolved stack: {error}") from error
    if result.returncode != 0:
        detail = result.stderr.strip() or "run it here to see why"
        raise ConfigUnavailable(
            f"`docker compose config` exited with status {result.returncode}; {detail}"
        )
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ConfigUnavailable(f"the resolved stack is not JSON: {error}") from error


def environment_value(service: dict, name: str) -> str | None:
    """One environment value of a resolved service.

    Compose writes `environment` as a map in some versions and as a list of
    `NAME=value` entries in others, and both shapes carry the same value.
    """
    environment = service.get("environment")
    if isinstance(environment, dict):
        value = environment.get(name)
        return value if isinstance(value, str) else None
    if isinstance(environment, list):
        for entry in environment:
            if isinstance(entry, str) and entry.startswith(f"{name}="):
                return entry[len(name) + 1 :]
    return None


def host_model_path(stack: dict) -> str | None:
    """The host file the extract service will open, or None when it is not in the stack."""
    service = stack.get("services", {}).get("extract")
    if not isinstance(service, dict):
        return None
    container = environment_value(service, MODEL_ARG)
    if not container:
        raise ConfigUnavailable(f"the resolved extract service has no {MODEL_ARG} value")
    for volume in service.get("volumes", []):
        if not isinstance(volume, dict) or volume.get("type") != "bind":
            continue
        source, target = volume.get("source"), volume.get("target")
        if not isinstance(source, str) or not isinstance(target, str):
            continue
        # The target must be a whole path segment, so a mount at `/models` is not
        # read as the parent of a file at `/models-other/x.gguf`.
        if container.startswith(f"{target}/"):
            return str(Path(source) / container[len(target) + 1 :])
    raise ConfigUnavailable(f"the resolved extract service binds nothing at {container}")


def missing_message(model: str) -> str:
    """What the operator sees.

    The fetch command is printed only for the model the project ships a source
    for. For any other name it would name a file that repository does not hold,
    which is worse than no suggestion at all.
    """
    if Path(model).name == DEFAULT_NAME:
        return f"error: missing extract model {model}; fetch it with `{FETCH_COMMAND}`"
    return (
        f"error: missing extract model {model}; the stack names a model other than the "
        f"shipped {DEFAULT_NAME}, so place that file in the models directory Compose mounts"
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--overlay", required=True, help="the arch overlay, such as compose.arm64.yml")
    parser.add_argument("--profile", help="the profile the start uses, if any")
    arguments = parser.parse_args(argv)
    try:
        stack = resolved_stack(arguments.overlay, arguments.profile)
        model = host_model_path(stack)
    except ConfigUnavailable as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if model is None or Path(model).is_file():
        return 0
    print(missing_message(model), file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
