#!/usr/bin/env python3
"""CI runs every suite that `make scripts-check` runs, and nothing else.

`mk/scripts.mk` names the suites as the prerequisites of `scripts-check`. The CI
workflow runs each one as its own step, so a failing suite shows up as its own
red status. A suite added to the aggregate with no step runs on a developer's
machine and nowhere else; that happened to `scripts-check-gate-install` and
`scripts-check-quick`. This check compares the two lists.

    python3 scripts/ci_parity_check.py
    python3 scripts/ci_parity_check.py --self-test

Exit codes: 0 the lists agree, 1 they differ, 2 a list could not be read.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
AGGREGATE_FILE = ROOT / "mk" / "scripts.mk"
WORKFLOW_FILE = ROOT / ".github" / "workflows" / "ci.yml"

AGGREGATE = re.compile(r"^scripts-check:((?:[^\n]*\\\n)*[^\n]*)", re.MULTILINE)
SUITE = re.compile(r"scripts-check-[\w-]+")
STEP = re.compile(r"^\s+run: make (scripts-check-[\w-]+)\s*$", re.MULTILINE)


def aggregate_suites(makefile_text: str) -> list[str]:
    """The suites `scripts-check` depends on, in order."""
    found = AGGREGATE.search(makefile_text)
    if found is None:
        raise ValueError("no `scripts-check:` rule found")
    return SUITE.findall(found.group(1).replace("\\\n", " "))


def workflow_suites(workflow_text: str) -> list[str]:
    """The suites the workflow runs as `run: make scripts-check-...` steps, in order."""
    return STEP.findall(workflow_text)


def differences(aggregate: list[str], workflow: list[str]) -> list[str]:
    problems = [f"{name}: in scripts-check, with no CI step" for name in aggregate if name not in workflow]
    problems += [f"{name}: a CI step, but not in scripts-check" for name in workflow if name not in aggregate]
    problems += [f"{name}: {workflow.count(name)} CI steps" for name in sorted(set(workflow)) if workflow.count(name) > 1]
    return problems


def self_test() -> int:
    mk = "scripts-check: scripts-check-a \\\n\tscripts-check-b scripts-check-c\n\nscripts-check-a:\n\ttrue\n"
    step = "      - name: x\n        run: make scripts-check-{}\n"
    cases = [
        ("agree", ["a", "b", "c"], []),
        ("a suite with no step", ["a", "b"], ["scripts-check-c: in scripts-check, with no CI step"]),
        ("a step with no suite", ["a", "b", "c", "d"], ["scripts-check-d: a CI step, but not in scripts-check"]),
        ("a repeated step", ["a", "b", "c", "c"], ["scripts-check-c: 2 CI steps"]),
    ]
    failed = 0
    for label, steps, expected in cases:
        text = "".join(step.format(name) for name in steps) + "        run: make scripts-check\n"
        got = differences(aggregate_suites(mk), workflow_suites(text))
        if got != expected:
            failed += 1
            print(f"ci-parity self-test: {label}: expected {expected}, got {got}")
    if aggregate_suites(mk) != ["scripts-check-a", "scripts-check-b", "scripts-check-c"]:
        failed += 1
        print("ci-parity self-test: a continued prerequisite line was not read whole")
    print("ci-parity self-test: failed" if failed else "ci-parity self-test: all passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    try:
        aggregate = aggregate_suites(AGGREGATE_FILE.read_text())
        workflow = workflow_suites(WORKFLOW_FILE.read_text())
    except (OSError, ValueError) as error:
        print(f"ci-parity: {error}", file=sys.stderr)
        return 2
    problems = differences(aggregate, workflow)
    for problem in problems:
        print(f"ci-parity: {problem}")
    if not problems:
        print(f"ci-parity: {len(aggregate)} suites, each with one CI step")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
