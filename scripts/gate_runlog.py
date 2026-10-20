"""A history of gate runs: one JSON line per run or replay, in `runs.jsonl` beside the cache.

The cache keeps the newest verdict per command and prunes the rest, so it cannot say
how long anyone waited on the gate over a day. This file can. Each line records the
worktree the gate ran in, the branch, the command, when it started and finished, and whether
a terminal was attached, which tells a run someone typed from a run a program started. Each
line also names the tree fingerprint and the machine load when the run started. A
`coverage-fast` run the Done check let go on also records the seconds it was estimated to take,
and every run records the test seconds nextest reported, when its log has them.

A failed append prints a warning and changes nothing else: the gate's exit code is the
command's own.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from gate_tree import cache_root

RUNS_FILE = "runs.jsonl"
ARGV_KEEP = 50
ARG_CHARS = 500
GIT_SECONDS = 10


def _rev_parse(root: str, *args: str) -> str | None:
    """One `git rev-parse` answer for the checkout at `root`, or None when git cannot give it."""
    try:
        done = subprocess.run(
            ["git", "-C", root, "rev-parse", *args],
            capture_output=True,
            text=True,
            check=False,
            timeout=GIT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    answer = done.stdout.strip()
    return answer if done.returncode == 0 and answer else None


def _terminal_attached() -> bool:
    try:
        return os.isatty(0)
    except OSError:
        return False


def machine_load() -> dict[str, Any]:
    """The 1-minute load average and the CPU count; `avg1` is None when the OS cannot give it."""
    try:
        avg1: float | None = round(os.getloadavg()[0], 2)
    except OSError:
        avg1 = None
    return {"avg1": avg1, "cpus": os.cpu_count()}


def append_run(root: str, run: dict[str, Any]) -> None:
    """Append one line describing `run`, which holds kind, argv, started, finished, exit, replayed.

    `common_dir` is the resolved git directory every worktree of one repository shares, so
    two worktrees of the same repository can be told apart from a temporary one.
    """
    try:
        common = _rev_parse(root, "--git-common-dir")
        line = {
            "root": str(Path(root).resolve()),
            "common_dir": str((Path(root) / common).resolve()) if common else None,
            "branch": _rev_parse(root, "--abbrev-ref", "HEAD"),
            "kind": run["kind"],
            "started": run["started"],
            "finished": run["finished"],
            "exit": run["exit"],
            "argv": [str(arg)[:ARG_CHARS] for arg in run["argv"][:ARGV_KEEP]],
            "replayed": run["replayed"],
            "interactive": _terminal_attached(),
            "estimate_s": run.get("estimate_s"),
            "tree": run.get("tree"),
            "load": run.get("load"),
            "test_s": run.get("test_s"),
        }
        path = cache_root() / RUNS_FILE
        path.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        try:
            os.write(descriptor, (json.dumps(line) + "\n").encode())
        finally:
            os.close(descriptor)
    except Exception as error:  # noqa: BLE001 - a history must never fail the gate
        print(f"gate: warning: could not append to the run log: {error}", file=sys.stderr)
