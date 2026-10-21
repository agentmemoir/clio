#!/usr/bin/env python3
"""CI runs every suite that `make scripts-check` runs, and nothing else.

`mk/scripts.mk` names the suites as the prerequisites of `scripts-check`. The CI
workflow runs each one as its own step, so a failing suite shows up as its own
red status. A suite added to the aggregate with no step runs on a developer's
machine and nowhere else; that happened to `scripts-check-gate-install` and
`scripts-check-quick`. This check compares the two lists.

The rule is per job. Every suite in `scripts-check` has a step in some job, no
step runs a suite that is not in `scripts-check`, and no suite runs in two
steps of the same job. A step is the unit: a step that runs two commands of
the same suite runs that suite once. A suite that runs in two different jobs
is a deliberate cost, so those suites are named in `ALLOWED_MULTI_JOB_SUITES`
and a new one has to be added on purpose.

This file holds the rules, the difference and cross-job reporting, the command
line and the self-test. The reading and the parsing live in
`scripts/ci_parity_workflow.py`, which was split out of this file to keep each
file under the 450-line limit.

    python3 scripts/ci_parity_check.py [--workflow PATH]
    python3 scripts/ci_parity_check.py --self-test

Exit codes: 0 the lists agree, 1 they differ, 2 a list could not be read.
"""

from __future__ import annotations

import sys
from pathlib import Path

from ci_parity_workflow import aggregate_suites, suite_commands, workflow_suites

ROOT = Path(__file__).resolve().parent.parent
AGGREGATE_FILE = ROOT / "mk" / "scripts.mk"
WORKFLOW_FILE = ROOT / ".github" / "workflows" / "ci.yml"

#: The suites that are allowed to run in more than one job. A second job runs a
#: suite when it checks that suite on another operating system, and that is a
#: deliberate cost rather than a mistake. The list is the record of that
#: choice, so a suite that runs in two jobs and is not named here is a finding.
ALLOWED_MULTI_JOB_SUITES = ("scripts-check-gate-install",)


def multi_job_suites(workflow: list[tuple[str, list[str]]]) -> list[tuple[str, list[str]]]:
    """The suites that run in more than one job, with the jobs that run them."""
    seen: dict[str, list[str]] = {}
    for job, suites in workflow:
        for name in suites:
            jobs = seen.setdefault(name, [])
            if job not in jobs:
                jobs.append(job)
    return [(name, jobs) for name, jobs in sorted(seen.items()) if len(jobs) > 1]


def differences(
    aggregate: list[str],
    workflow: list[tuple[str, list[str]]],
    allowed: tuple[str, ...] = ALLOWED_MULTI_JOB_SUITES,
) -> list[str]:
    """The ways the workflow and the aggregate disagree."""
    steps = [suite for _, suites in workflow for suite in suites]
    problems = [f"{name}: in scripts-check, with no CI step" for name in aggregate if name not in steps]
    problems += [f"{name}: a CI step, but not in scripts-check" for name in sorted(set(steps)) if name not in aggregate]
    for job, suites in workflow:
        for name in sorted(set(suites)):
            if suites.count(name) > 1:
                problems.append(f"{name}: {suites.count(name)} CI steps in job {job}")
    for name, jobs in multi_job_suites(workflow):
        if name not in allowed:
            problems.append(f"{name}: runs in more than one job ({', '.join(jobs)}) and is not listed as allowed to")
    return problems


#: The ways a step can be written in the synthetic workflow of the self-test.
#: The names are the ways a real workflow can spell the same step.
SHAPES = {
    "plain": "      - name: x\n        run: make scripts-check-{suite}\n",
    "block": "      - name: x\n        run: |\n          make scripts-check-{suite}\n",
    "odd": "        - name: x\n          run: make scripts-check-{suite}\n",
    "flag": "      - name: x\n        run: make -C . scripts-check-{suite}\n",
    "comment": "      - name: x\n        run: make scripts-check-{suite}  # runs the suite\n",
    "both": "      - name: x\n        run: make -C . scripts-check-{suite}  # runs the suite\n",
    "twice": "      - name: x\n        run: |\n          make scripts-check-{suite}\n          make scripts-check-{suite}\n",
}


def job_text(name: str, suites: str, shape: str = "plain") -> str:
    """A job of the synthetic workflow, with one step per suite."""
    text = f"  {name}:\n    steps:\n"
    for suite in suites:
        text += SHAPES[shape].format(suite=suite)
    return text


def self_test() -> int:
    mk = (
        "scripts-check: scripts-check-a \\\n\tscripts-check-b scripts-check-c\n\n"
        "scripts-check-a:\n\tpython3 scripts/test_gate_install.py\n"
        "\tpython3 scripts/test_gate_install_profiles.py\n"
    )
    commands = suite_commands(mk)
    no_c = "scripts-check-c: in scripts-check, with no CI step"
    no_b = "scripts-check-b: in scripts-check, with no CI step"
    two_c = "scripts-check-c: 2 CI steps in job check"
    extra_d = "scripts-check-d: a CI step, but not in scripts-check"
    two_c_jobs = "scripts-check-c: runs in more than one job (check, macos) and is not listed as allowed to"
    two_a_jobs = "scripts-check-a: runs in more than one job (check, macos) and is not listed as allowed to"
    macos = (
        "  macos:\n    steps:\n      - name: x\n        run: |\n"
        "          python3 scripts/test_gate_install.py\n"
        "          python3 scripts/test_gate_install_profiles.py\n"
    )
    cases = [
        ("agree", job_text("check", "abc"), [], ()),
        ("a suite with no step", job_text("check", "ab"), [no_c], ()),
        ("a step with no suite", job_text("check", "abcd"), [extra_d], ()),
        ("a repeated step in one job", job_text("check", "abcc"), [two_c], ()),
        ("one step with two commands of a suite", job_text("check", "abc", "twice"), [], ()),
        ("the same suite in two jobs, allowed", job_text("check", "abc") + job_text("macos", "c"), [], ("scripts-check-c",)),
        ("the same suite in two jobs, not allowed", job_text("check", "abc") + job_text("macos", "c"), [two_c_jobs], ()),
        ("a suite run by its own commands in a second job", job_text("check", "abc") + macos, [], ("scripts-check-a",)),
        ("a suite run by its own commands in a second job, not allowed", job_text("check", "abc") + macos, [two_a_jobs], ()),
        ("a suite in a block step", job_text("check", "abc", "block"), [], ()),
        ("a repeated step in a block", job_text("check", "acc", "block"), [no_b, two_c], ()),
        ("a step at an odd indent", job_text("check", "abc", "odd"), [], ()),
        ("make with a flag", job_text("check", "abc", "flag"), [], ()),
        ("a command with a trailing comment", job_text("check", "abc", "comment"), [], ()),
        ("a suite with no step at an odd indent", job_text("check", "ab", "odd"), [no_c], ()),
        ("a suite with no step, flagged and commented", job_text("check", "ab", "both"), [no_c], ()),
    ]
    failed = 0
    for label, text, expected, allowed in cases:
        text = "jobs:\n" + text + "        run: make scripts-check\n"
        got = differences(aggregate_suites(mk), workflow_suites(text, commands), allowed)
        if got != expected:
            failed += 1
            print(f"ci-parity self-test: {label}: expected {expected}, got {got}")
    multi = multi_job_suites(
        workflow_suites("jobs:\n" + job_text("check", "abc", "twice") + job_text("macos", "c"), commands)
    )
    if multi != [("scripts-check-c", ["check", "macos"])]:
        failed += 1
        print(f"ci-parity self-test: the suites that run in more than one job: expected [('scripts-check-c', ['check', 'macos'])], got {multi}")
    multi = multi_job_suites(workflow_suites("jobs:\n" + job_text("check", "abc") + macos, commands))
    if multi != [("scripts-check-a", ["check", "macos"])]:
        failed += 1
        print(f"ci-parity self-test: a suite run by its own commands in two jobs: expected [('scripts-check-a', ['check', 'macos'])], got {multi}")
    if aggregate_suites(mk) != ["scripts-check-a", "scripts-check-b", "scripts-check-c"]:
        failed += 1
        print("ci-parity self-test: a continued prerequisite line was not read whole")
    print("ci-parity self-test: failed" if failed else "ci-parity self-test: all passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    workflow_file = WORKFLOW_FILE
    if len(argv) == 2 and argv[0] == "--workflow":
        workflow_file = Path(argv[1])
    elif argv:
        print("usage: ci_parity_check.py [--workflow PATH]", file=sys.stderr)
        return 2
    try:
        makefile_text = AGGREGATE_FILE.read_text()
        aggregate = aggregate_suites(makefile_text)
        commands = suite_commands(makefile_text)
        workflow = workflow_suites(workflow_file.read_text(), commands)
    except (OSError, ValueError) as error:
        print(f"ci-parity: {error}", file=sys.stderr)
        return 2
    problems = differences(aggregate, workflow)
    for problem in problems:
        print(f"ci-parity: {problem}")
    if not problems:
        print(f"ci-parity: {len(aggregate)} suites, each with a CI step")
        multi = multi_job_suites(workflow)
        if multi:
            names = ", ".join(f"{name} ({', '.join(jobs)})" for name, jobs in multi)
            word = "suite runs" if len(multi) == 1 else "suites run"
            print(f"ci-parity: {len(multi)} {word} in more than one job: {names}")
        else:
            print("ci-parity: no suite runs in more than one job")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
