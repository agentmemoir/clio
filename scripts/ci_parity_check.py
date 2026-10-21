#!/usr/bin/env python3
"""CI runs every suite that `make scripts-check` runs, and nothing else.

`mk/scripts.mk` names the suites as the prerequisites of `scripts-check`. The CI
workflow runs each one as its own step, so a failing suite shows up as its own
red status. A suite added to the aggregate with no step runs on a developer's
machine and nowhere else; that happened to `scripts-check-gate-install` and
`scripts-check-quick`. This check compares the two lists.

The rule is per job. Every suite in `scripts-check` has a step in some job, no
step runs a suite that is not in `scripts-check`, and no suite runs twice in
the same job. The same suite in two different jobs is not a problem: a suite
that runs on Linux and on macOS runs on two runners, not twice on one runner.

A step that runs a suite in a `run: |` block counts as a step too, so a suite
cannot be hidden in a block from this check.

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

# A job header is a line of exactly two spaces of indent, a name, and a colon,
# under the top-level `jobs:` key. The `on:` and `env:` blocks above `jobs:`
# hold keys at the same indent, so the search starts at `jobs:` and stops at
# the next top-level key.
JOBS_KEY = re.compile(r"^jobs:[ \t]*$")
JOB_HEADER = re.compile(r"^  ([A-Za-z0-9_.-]+):[ \t]*(?:#.*)?$")
TOP_LEVEL_KEY = re.compile(r"^[A-Za-z0-9_.-]+:")

# A step's `run:` key sits at eight spaces of indent, under the `- ` marker at
# six. The body of a `run: |` block sits deeper than that, so a body line is
# never read as a key of its own. A step written on one line reads as
# `- run: ...` at six.
RUN_KEY = re.compile(r"^ {8}run:[ \t]*(.*)$")
INLINE_RUN_KEY = re.compile(r"^ {6}- run:[ \t]*(.*)$")
BLOCK_MARKER = re.compile(r"^[|>][-+]?\d*$")

# A command that runs one suite.
MAKE_SUITE = re.compile(r"^make[ \t]+(scripts-check-[\w-]+)\b")


def aggregate_suites(makefile_text: str) -> list[str]:
    """The suites `scripts-check` depends on, in order."""
    found = AGGREGATE.search(makefile_text)
    if found is None:
        raise ValueError("no `scripts-check:` rule found")
    return SUITE.findall(found.group(1).replace("\\\n", " "))


def indent_of(line: str) -> int:
    """The number of spaces in front of a line."""
    return len(line) - len(line.lstrip(" "))


def split_jobs(workflow_text: str) -> list[tuple[str, list[str]]]:
    """The jobs of the workflow, each with its name and its own lines."""
    jobs: list[tuple[str, list[str]]] = []
    current: list[str] | None = None
    in_jobs = False
    for line in workflow_text.splitlines():
        if not in_jobs:
            in_jobs = JOBS_KEY.match(line) is not None
            continue
        if line and not line[0].isspace() and TOP_LEVEL_KEY.match(line):
            break
        header = JOB_HEADER.match(line)
        if header is not None:
            current = []
            jobs.append((header.group(1), current))
            continue
        if current is not None:
            current.append(line)
    return jobs


def run_command(line: str) -> tuple[str, int] | None:
    """The command of a step's `run:` key, with the indent of that key."""
    for pattern in (RUN_KEY, INLINE_RUN_KEY):
        found = pattern.match(line)
        if found is not None:
            return found.group(1).strip(), indent_of(line)
    return None


def job_suites(job_lines: list[str]) -> list[str]:
    """The suites one job runs, in order."""
    suites: list[str] = []
    index = 0
    while index < len(job_lines):
        run = run_command(job_lines[index])
        if run is None:
            index += 1
            continue
        value, base = run
        if BLOCK_MARKER.match(value):
            index += 1
            while index < len(job_lines):
                body = job_lines[index]
                if body.strip() == "":
                    index += 1
                    continue
                if indent_of(body) <= base:
                    break
                found = MAKE_SUITE.match(body.strip())
                if found is not None:
                    suites.append(found.group(1))
                index += 1
            continue
        found = MAKE_SUITE.match(value)
        if found is not None:
            suites.append(found.group(1))
        index += 1
    return suites


def workflow_suites(workflow_text: str) -> list[tuple[str, list[str]]]:
    """The suites the workflow runs, per job, in order."""
    return [(name, job_suites(lines)) for name, lines in split_jobs(workflow_text)]


def differences(aggregate: list[str], workflow: list[tuple[str, list[str]]]) -> list[str]:
    """The ways the workflow and the aggregate disagree."""
    steps = [suite for _, suites in workflow for suite in suites]
    problems = [f"{name}: in scripts-check, with no CI step" for name in aggregate if name not in steps]
    problems += [f"{name}: a CI step, but not in scripts-check" for name in sorted(set(steps)) if name not in aggregate]
    for job, suites in workflow:
        for name in sorted(set(suites)):
            if suites.count(name) > 1:
                problems.append(f"{name}: {suites.count(name)} CI steps in job {job}")
    return problems


def job_text(name: str, suites: list[str], block: bool = False) -> str:
    """A job of the synthetic workflow, with one step per suite."""
    text = f"  {name}:\n    steps:\n"
    for suite in suites:
        if block:
            text += f"      - name: x\n        run: |\n          make scripts-check-{suite}\n"
        else:
            text += f"      - name: x\n        run: make scripts-check-{suite}\n"
    return text


def self_test() -> int:
    mk = "scripts-check: scripts-check-a \\\n\tscripts-check-b scripts-check-c\n\nscripts-check-a:\n\ttrue\n"
    cases = [
        ("agree", job_text("check", ["a", "b", "c"]), []),
        ("a suite with no step", job_text("check", ["a", "b"]), ["scripts-check-c: in scripts-check, with no CI step"]),
        (
            "a step with no suite",
            job_text("check", ["a", "b", "c", "d"]),
            ["scripts-check-d: a CI step, but not in scripts-check"],
        ),
        (
            "a repeated step in one job",
            job_text("check", ["a", "b", "c", "c"]),
            ["scripts-check-c: 2 CI steps in job check"],
        ),
        ("the same suite in two jobs", job_text("check", ["a", "b", "c"]) + job_text("macos", ["c"]), []),
        ("a suite in a block step", job_text("check", ["a", "b", "c"], block=True), []),
        (
            "a repeated step in a block",
            job_text("check", ["a", "c", "c"], block=True),
            [
                "scripts-check-b: in scripts-check, with no CI step",
                "scripts-check-c: 2 CI steps in job check",
            ],
        ),
    ]
    failed = 0
    for label, text, expected in cases:
        text = "jobs:\n" + text + "        run: make scripts-check\n"
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
        print(f"ci-parity: {len(aggregate)} suites, each with a CI step")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
