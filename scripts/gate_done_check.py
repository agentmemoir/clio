"""The Done check at the gate: a program's `make coverage-fast` runs once, and only when it fits.

`make coverage-fast` is an optional local check of the changed files' coverage; CI judges the
same files on every pull request. Before a run starts, this module decides whether a run with
no terminal attached may go on.

A person at a terminal is never refused. Without a terminal, the run is refused when this
worktree already ran it on this branch, when Quick's closure estimate is over the budget, and
when no estimate can be made. A replay of a stored verdict costs nothing, and the gate allows
it before this module is asked.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple

from gate_runlog import RUNS_FILE, _rev_parse, _terminal_attached
from gate_tree import cache_root

EXIT_REFUSED = 3
KIND = "coverage-fast"
ESTIMATE_TIMEOUT_S = 60
LEFT_TO_CI = "CI decides the coverage verdict; report it as left to CI."


class Decision(NamedTuple):
    reason: str | None
    estimate_s: float | None


ALLOW = Decision(None, None)


def _is_plan(argv: list[str]) -> bool:
    """`make coverage-fast PLAN=1` prints the selection and runs nothing; any value of PLAN does that."""
    return any(arg.startswith("PLAN=") and arg != "PLAN=" for arg in argv)


def _base(argv: list[str]) -> str:
    """The BASE the make target measures against; the Makefile's own default is `master`."""
    for arg in argv:
        if arg.startswith("BASE="):
            return arg[len("BASE="):] or "master"
    return "master"


def _estimate(root: str, base: str) -> tuple[dict | None, str]:
    """Quick's closure estimate as a dict, or None with the reason there is none."""
    script = Path(root) / "scripts" / "quick.py"
    command = [sys.executable, str(script), "--base", base, "--plan", "--closure", "--json"]
    try:
        done = subprocess.run(command, cwd=root, capture_output=True, text=True, check=False, timeout=ESTIMATE_TIMEOUT_S)
        if done.returncode != 0:
            return None, (done.stderr.strip().splitlines() or [f"exit {done.returncode}"])[-1]
        doc = json.loads(done.stdout)
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        return None, str(error)
    return (doc, "") if isinstance(doc, dict) else (None, "unreadable answer")


def _ran_before(root: str, branch: str | None) -> bool:
    """True when this worktree already ran the check for real on this branch."""
    here = str(Path(root).resolve())
    try:
        text = (cache_root() / RUNS_FILE).read_text()
    except OSError:
        return False
    for line in text.splitlines():
        try:
            run = json.loads(line)
        except ValueError:
            continue
        if not isinstance(run, dict) or run.get("kind") != KIND or run.get("replayed"):
            continue
        argv = run.get("argv")
        if isinstance(argv, list) and not _is_plan([str(arg) for arg in argv]):
            if run.get("root") == here and run.get("branch") == branch:
                return True
    return False


def decide(kind: str, argv: list[str], root: str) -> Decision:
    """Whether this run may start, and the time it is expected to take when it may."""
    if kind != KIND or _is_plan(argv) or _terminal_attached():
        return ALLOW
    branch = _rev_parse(root, "--abbrev-ref", "HEAD")
    if _ran_before(root, branch):
        return Decision(f"it already ran once on branch {branch} in this worktree. {LEFT_TO_CI}", None)
    doc, why = _estimate(root, _base(argv))
    if doc is None:
        return Decision(f"its length cannot be estimated ({why}). {LEFT_TO_CI}", None)
    seconds = doc.get("done_seconds")
    if not isinstance(seconds, (int, float)):
        return Decision(f"no per-test times exist to estimate it from, so its length is unknown. {LEFT_TO_CI}", None)
    if not doc.get("within"):
        budget = doc.get("budget_seconds", 0)
        return Decision(f"it is estimated at {seconds:.0f} s, over the {budget:.0f} s budget. {LEFT_TO_CI}", None)
    return Decision(None, float(seconds))
