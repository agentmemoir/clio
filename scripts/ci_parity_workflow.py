"""The reading and the parsing of the CI parity check.

Split out of ci_parity_check.py to keep each file under the 450-line limit.
This module holds the reading: the makefile that names the suites, the
workflow text, the jobs of that workflow, the steps of a job, the suite a
command runs, and the suite a step runs. The checker holds the rules, the
reporting, the command line and the self-test.

Nothing here decides what a problem is. A suite is read by its make target, or
by the command that target runs, with or without flags in front of the target
and with or without a comment after it. The workflow is read by its shape and
not by its columns: a job is a key under `jobs:`, its steps are the block
sequence under its `steps:` key, and a step's `run:` key sits at the indent of
that step's first key. A suite must not go invisible because someone
reindented the workflow.
"""

from __future__ import annotations

import re

# The `scripts-check:` rule of the makefile, and the suite names in it.
AGGREGATE = re.compile(r"^scripts-check:((?:[^\n]*\\\n)*[^\n]*)", re.MULTILINE)
SUITE = re.compile(r"scripts-check-[\w-]+")

# The top-level `jobs:` key, the key that ends the jobs block, and the name of
# a job. The indent of the job block is read from the file, so a job header is
# a key at that indent and not at a fixed column.
JOBS_KEY = re.compile(r"^jobs:[ \t]*$")
TOP_LEVEL_KEY = re.compile(r"^[A-Za-z0-9_.-]+:")
JOB_NAME = re.compile(r"^([A-Za-z0-9_.\"'-]+):")

# The `steps:` key of a job and a `run:` key. A `run:` key is read wherever it
# sits, so a step that is written at another indent is still read.
STEPS_KEY = re.compile(r"^([ ]*)steps:[ \t]*$")
RUN_KEY_ANY = re.compile(r"^([ ]*)(?:-([ ]+))?run:[ \t]*(.*)$")
BLOCK_MARKER = re.compile(r"^[|>][-+]?\d*$")

# A target of the aggregate, and the commands its recipe runs. A job that runs
# the command of a suite without the target still runs the suite.
TARGET = re.compile(r"^(scripts-check-[\w-]+):[ \t]*$")

# The make programs a job can call, the make options that take a value of their
# own, and the wrappers a job can put in front of make. A job can put flags in
# front of the target, so the target is the first word that is neither a flag,
# nor the value of a flag, nor a variable assignment.
MAKE_PROGRAMS = ("make", "gmake", "mingw32-make")
MAKE_VALUE_FLAGS = ("-C", "-f", "-I", "-o", "-W", "--directory", "--file", "--include-dir")
MAKE_JOBS_FLAGS = ("-j", "--jobs")
MAKE_WRAPPERS = ("sudo", "env")

# A make target that names a suite, and the shell operators that start a new
# command.
SUITE_TARGET = re.compile(r"^(scripts-check-[\w-]+)$")
SEGMENT = re.compile(r"\s*(?:\|\||&&|;|\|)\s*")


def aggregate_suites(makefile_text: str) -> list[str]:
    """The suites `scripts-check` depends on, in order."""
    found = AGGREGATE.search(makefile_text)
    if found is None:
        raise ValueError("no `scripts-check:` rule found")
    return SUITE.findall(found.group(1).replace("\\\n", " "))


def indent_of(line: str) -> int:
    """The number of spaces in front of a line."""
    return len(line) - len(line.lstrip(" "))


def is_skippable(line: str) -> bool:
    """True for a blank line and for a comment line."""
    text = line.strip()
    return text == "" or text.startswith("#")


def strip_comment(command: str) -> str:
    """The command without a trailing comment.

    A `#` starts a comment in a shell command when it is not inside a quoted
    string and it follows a space, or when it is the first character of the
    command.
    """
    quote = ""
    for index, char in enumerate(command):
        if quote:
            if char == quote:
                quote = ""
            continue
        if char in "'\"":
            quote = char
            continue
        if char == "#" and (index == 0 or command[index - 1].isspace()):
            return command[:index]
    return command


def normalise(command: str) -> str:
    """A command without its comment, with its spaces collapsed."""
    return " ".join(strip_comment(command).split())


def suite_commands(makefile_text: str) -> dict[str, str]:
    """The command each suite target runs, mapped to the suite name.

    A job that runs the command of a suite runs the suite, even when it does
    not go through the make target. The macOS job does that, because the
    Makefile resolves a Rust toolchain the runner does not ship.
    """
    commands: dict[str, str] = {}
    lines = makefile_text.splitlines()
    for index, line in enumerate(lines):
        found = TARGET.match(line)
        if found is None:
            continue
        suite = found.group(1)
        for body in lines[index + 1:]:
            if not body.startswith("\t"):
                break
            commands.setdefault(normalise(body), suite)
    return commands


def split_jobs(workflow_text: str) -> list[tuple[str, list[str]]]:
    """The jobs of the workflow, each with its name and its own lines.

    The indent of the job block is read from the first line under `jobs:`, so
    a job header is a key at that indent and not at a fixed column. The lines
    of a job are the lines under its header.
    """
    lines = workflow_text.splitlines()
    start = next((i for i, line in enumerate(lines) if JOBS_KEY.match(line)), None)
    if start is None:
        return []
    block: list[str] = []
    for line in lines[start + 1:]:
        if line and not line[0].isspace() and TOP_LEVEL_KEY.match(line):
            break
        block.append(line)
    job_indent = next((indent_of(line) for line in block if not is_skippable(line)), None)
    if job_indent is None:
        return []
    jobs: list[tuple[str, list[str]]] = []
    current: list[str] | None = None
    for line in block:
        if not is_skippable(line) and indent_of(line) == job_indent and JOB_NAME.match(line.strip()):
            current = []
            jobs.append((line.strip().split(":", 1)[0].strip("\"'"), current))
            continue
        if current is not None:
            current.append(line)
    return jobs


def scan_run_keys(lines: list[str]) -> list[list[str]]:
    """The commands of each `run:` key, at any indent, one list per step.

    A `run:` key is read wherever it sits, so a step that is written at
    another indent is still read. The body of a block is skipped, so a line
    inside a block is never read as a key of its own. The commands of a step
    stay together, so a step that runs two commands of a suite is one step.
    """
    steps: list[list[str]] = []
    index = 0
    while index < len(lines):
        found = RUN_KEY_ANY.match(lines[index])
        index += 1
        if found is None:
            continue
        base = len(found.group(1)) + (1 + len(found.group(2)) if found.group(2) else 0)
        value = found.group(3).strip()
        step = [value]
        if BLOCK_MARKER.match(value):
            while index < len(lines) and (is_skippable(lines[index]) or indent_of(lines[index]) > base):
                step.append(lines[index].strip())
                index += 1
        steps.append(step)
    return steps


def job_commands(job_lines: list[str]) -> list[list[str]]:
    """The steps of a job, each with the commands it runs.

    The `steps:` key is a child of the job, so it sits at the indent of the
    job's other children. The lines under it hold the steps, and their `run:`
    keys are read at any indent, so a step that is written at another indent
    is still read. A job with no `steps:` key is read whole, so a shape this
    reader does not know does not hide a suite.
    """
    children = [indent_of(line) for line in job_lines if not is_skippable(line)]
    if not children:
        return []
    child_indent = min(children)
    for index, line in enumerate(job_lines):
        found = STEPS_KEY.match(line)
        if found is not None and len(found.group(1)) == child_indent:
            return scan_run_keys(job_lines[index:])
    return scan_run_keys(job_lines)


def make_target(command: str) -> str | None:
    """The make target a command runs, or None when it runs no make target.

    A job can put a wrapper in front of make, and it can put flags in front of
    the target, so the target is not always the second word.
    """
    for segment in SEGMENT.split(command):
        words = segment.split()
        index = 0
        while index < len(words):
            program = words[index].rsplit("/", 1)[-1]
            if program in MAKE_WRAPPERS:
                index += 1
                while index < len(words) and ("-" in words[index] or "=" in words[index]):
                    index += 1
                continue
            if program not in MAKE_PROGRAMS:
                break
            index += 1
            while index < len(words):
                word = words[index]
                if word.startswith("-"):
                    if word in MAKE_VALUE_FLAGS:
                        index += 2
                        continue
                    if word in MAKE_JOBS_FLAGS and index + 1 < len(words) and words[index + 1].isdigit():
                        index += 2
                        continue
                    index += 1
                    continue
                if "=" in word:
                    index += 1
                    continue
                found = SUITE_TARGET.match(word)
                return found.group(1) if found is not None else None
            return None
    return None


def suite_of(command: str, commands: dict[str, str]) -> str | None:
    """The suite a command runs, or None when it runs no suite."""
    text = normalise(command)
    found = make_target(text)
    if found is not None:
        return found
    for recipe, suite in commands.items():
        if text == recipe or text.startswith(recipe + " "):
            return suite
    return None


def step_suites(step: list[str], commands: dict[str, str]) -> list[str]:
    """The suites one step runs, each named once.

    A step is the unit: a step that runs two commands of a suite runs it once.
    """
    found: list[str] = []
    for command in step:
        suite = suite_of(command, commands)
        if suite is not None and suite not in found:
            found.append(suite)
    return found


def workflow_suites(
    workflow_text: str, commands: dict[str, str]
) -> list[tuple[str, list[str]]]:
    """The suites the workflow runs, per job, in order.

    A suite is named once per step that runs it, so a job that runs a suite in
    two steps names it twice and a step that runs it twice names it once.
    """
    suites: list[tuple[str, list[str]]] = []
    for name, lines in split_jobs(workflow_text):
        found: list[str] = []
        for step in job_commands(lines):
            found.extend(step_suites(step, commands))
        suites.append((name, found))
    return suites
