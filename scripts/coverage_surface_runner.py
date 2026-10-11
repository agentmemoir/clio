"""Reading the gate's own Python for the steps around a coverage run."""

from __future__ import annotations

import io
import re
import tokenize

from coverage_surface_invocations import Invocation, failure_mode

#: The report-boundary hygiene steps, and what removing each one costs. Rule 4.
HYGIENE_STEPS = {
    "drop_invalid_profraws": (
        "an unreadable profile buffer fails the whole merge, after every test has "
        "already passed"
    ),
    "prune_foreign_objects": (
        "the merge unions the objects this run did not build, so a file's line "
        "set and function count move with the directory rather than with the file"
    ),
}

#: Selected binaries must be removed before tests can rebuild them.
PRE_TEST_STEPS = {
    "drop_selected_objects": (
        "a selected package's earlier executable can keep its old line mapping "
        "beside the one this run builds"
    ),
}

#: The call that runs the tests and the call that writes the report. A function
#: holding one of each is a run, and rule 4 is about runs. A function holding only
#: one of them is a step or a test, not a run: the report step does the merge
#: without doing the hygiene because its caller already did, and the gate's own
#: self-test builds the command to assert what it contains rather than running it.
#:
#: The report side is the step rather than the builder, and that is deliberate: the
#: self-test calls `coverage_report_command` and neither hygiene step, so a rule
#: read from the builder alone would refuse the very file whose cases prove the
#: two steps work. A run that built the report command and executed it itself
#: would escape rule 4; it would also escape the test-step half, so neither side
#: would call it a run.
REPORT_CALLS = ("write_coverage_report",)
TEST_STEP_CALLS = ("run_coverage_tests", "coverage_test_command")


def _prose_spans(tokens) -> list[tuple[int, int, int, int]]:
    """The character ranges carrying a comment or a docstring, as token positions.

    A docstring is the first statement of a module, a class or a function, so each
    `def` and each `class` opens a wait for one and the first statement that
    follows settles it: a string is that block's docstring, anything else means
    the block has none. A string that is a value rather than a statement settles
    the wait without being dropped, which is what keeps
    `TARGET_KEY = "CARGO_LLVM_COV_TARGET_DIR"` a pin rather than prose.

    A string written on the same line as its `def` arrives while the signature is
    still open, where it is a default value rather than a docstring, so it is
    kept. The frames are per block and are dropped as the tokenizer dedents, so a
    block that never reaches a docstring cannot claim the string that follows it.

    Ranges and not line numbers, because a comment shares its line with the code
    it follows. `prune_foreign_objects(directory, packages)  # and then the report`
    is a call with a note on it, and blanking the line to remove the note would
    remove the call and then report a run that does its job as one that does not.
    Positions are returned rather than applied here so the caller can blank to
    the same length and keep every line and column number true.
    """
    spans: list[tuple[int, int, int, int]] = []
    skip = (tokenize.NL, tokenize.INDENT, tokenize.ENDMARKER)
    # One frame per open block, as (its column, signature open, docstring still
    # possible). The module is the outermost frame and waits from the first token.
    waiting = [[-1, False, True]]
    for token in tokens:
        if token.type == tokenize.COMMENT:
            spans.append((token.start[0], token.start[1], token.end[0], token.end[1]))
            continue
        if token.type == tokenize.NEWLINE:
            # The first newline after a `def` or a `class` name ends that
            # signature. Every newline after it ends a statement inside the body,
            # and a statement is what settles the wait for a docstring.
            if waiting[-1][1]:
                waiting[-1][1] = False
            continue
        if token.type == tokenize.DEDENT:
            column = token.start[1]
            while len(waiting) > 1 and waiting[-1][0] > column:
                waiting.pop()
            continue
        if token.type in skip:
            continue
        if token.type == tokenize.NAME and token.string in ("def", "class"):
            waiting[-1][2] = False
            waiting.append([token.start[1], True, True])
            continue
        frame = waiting[-1]
        if frame[1]:
            # Still inside the signature, so this is a parameter or a default.
            continue
        if frame[2] and token.type == tokenize.STRING:
            spans.append((token.start[0], token.start[1], token.end[0], token.end[1]))
        frame[2] = False
    return spans


def strip_python_prose(text: str) -> str | None:
    """`text` without its comments and without any of its docstrings, or None.

    Both are prose about the code rather than the code, and reading them as the
    code is not hypothetical: the gate's test step carries `--no-report` and its
    docstring explains what the tool's `report` subcommand does, so the word
    `report` in that sentence made the step read as a report writer and the check
    reported two findings against the one function that is exactly right.

    Stripped with the tokenizer rather than by scanning for a triple quote,
    because a command literal is itself a run of string tokens and only the
    tokenizer knows which of them is a docstring.

    The result is the **same length as the input**, with prose replaced by spaces
    rather than deleted. Two things depend on that. A line number or a column in
    the stripped text is the one in the source, so a rule may report where it
    found something without carrying the raw text alongside to look it up in. And a
    comment shares its line with the code it follows, so blanking whole lines to
    remove prose would delete the code too and then report a run that does its job
    as one that does not.

    None when the tokenizer refuses the text, because a strip that failed leaves
    the comments in and a comment naming a protection is exactly what must not
    answer for one. The one caller that can still use the text reads the rules it
    wants from what it can see, and says so.
    """
    try:
        tokens = list(tokenize.generate_tokens(io.StringIO(text).readline))
    except (tokenize.TokenError, IndentationError, SyntaxError):
        return None
    chars = list(text)
    for start_row, start_col, end_row, end_col in _prose_spans(tokens):
        for row in range(start_row, end_row + 1):
            line = _line_start(chars, row)
            if line is None:
                continue
            first = start_col if row == start_row else 0
            if row == end_row:
                stop = line + end_col
            else:  # a row inside the span is blanked to its end, and no further
                stop = line
                while stop < len(chars) and chars[stop] != "\n":
                    stop += 1
            for index in range(line + first, min(stop, len(chars))):
                if chars[index] != "\n":
                    chars[index] = " "
    return "".join(chars)


def _line_start(chars: list[str], row: int) -> int | None:
    """The index of the first character of one-based `row`, or None if past the end."""
    if row == 1:
        return 0
    seen = 0
    for index, char in enumerate(chars):
        if char == "\n":
            seen += 1
            if seen == row - 1:
                return index + 1
    return None


def calls_in(text: str, name: str) -> list[tuple[int, str]]:
    """Every call to `name` in the file, as (the line it starts on, that line).

    The defining `def name(` line is not a call, and counting it would make every
    rule 4 finding pass on a function that defines the step and never uses it.
    """
    found = []
    for number, raw in enumerate(text.splitlines(), start=1):
        if f"{name}(" not in raw:
            continue
        if re.match(r"\s*def\s", raw) or re.match(r"\s*#", raw):
            continue
        found.append((number, raw.strip()))
    return found


def enclosing_functions(text: str) -> dict[int, str]:
    """The top-level function each line sits in, as {line number: function name}.

    Built by walking forward and remembering the last `def` seen, which is what
    rule 4 needs: a report call belongs to the run that wrote it, not to whichever
    function happens to appear first in the file. Two runs written one after the
    other would otherwise be read as one, and deleting a hygiene step from the
    first of them would look like the second still had it.
    """
    current = ""
    owners: dict[int, str] = {}
    for number, raw in enumerate(text.splitlines(), start=1):
        match = re.match(r"def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(", raw)
        if match is not None:
            current = match.group(1)
        owners[number] = current
    return owners


def _hygiene_problem(text: str) -> list[tuple[int, str, str]]:
    """Rule 4's findings, as (line, rule, message).

    Each run must remove old selected objects before tests, then drop unreadable buffers and foreign objects before the report; a function that only writes the report is not a run.
    """
    owners = enclosing_functions(text)
    runs = {
        owners.get(number, "")
        for name in TEST_STEP_CALLS
        for number, _ in calls_in(text, name)
    }
    runs.discard("")
    report_sites: dict[str, int] = {}
    for name in REPORT_CALLS:
        for number, _ in calls_in(text, name):
            owner = owners.get(number, "")
            if owner in runs:
                report_sites.setdefault(owner, number)
    problems: list[tuple[int, str, str]] = []
    for owner, line in sorted(report_sites.items(), key=lambda item: item[1]):
        test_sites = [
            number
            for name in TEST_STEP_CALLS
            for number, _ in calls_in(text, name)
            if owners.get(number, "") == owner
        ]
        for name, cost in PRE_TEST_STEPS.items():
            sites = [
                number
                for number, _ in calls_in(text, name)
                if owners.get(number, "") == owner
            ]
            if not sites:
                problems.append(
                    (
                        min(test_sites),
                        "hygiene",
                        f"`{owner}` runs instrumented tests but never calls `{name}`, "
                        f"so {cost}. The old object must be removed before the test "
                        "step builds a new one.",
                    )
                )
            elif min(sites) > min(test_sites):
                problems.append(
                    (
                        min(sites),
                        "hygiene",
                        f"`{owner}` runs instrumented tests on line {min(test_sites)} "
                        f"but calls `{name}` on line {min(sites)}, after them. The "
                        "old object can then remain beside the new build.",
                    )
                )
        for name, cost in HYGIENE_STEPS.items():
            sites = [number for number, _ in calls_in(text, name) if owners.get(number, "") == owner]
            if not sites:
                problems.append(
                    (
                        line,
                        "hygiene",
                        f"`{owner}` writes a coverage report but never calls "
                        f"`{name}`, so {cost}. That step is the only thing standing "
                        f"between the report and a wrong number, and removing the "
                        f"call leaves a run that reports the same thing as before "
                        f"while measuring something else.",
                    )
                )
            elif min(sites) > line:
                problems.append(
                    (
                        line,
                        "hygiene",
                        f"`{owner}` writes its coverage report on line {line} but "
                        f"calls `{name}` on line {min(sites)}, after it. The merge "
                        f"has already read everything the step removes by then, so "
                        f"the step no longer does what it is there for.",
                    )
                )
    return problems


def builder_body(text: str, name: str) -> tuple[int, str] | None:
    """One top-level `def name(...)` as (its line number, its text), or None.

    Read by name rather than by pattern, so renaming a builder is a finding here
    rather than a silently shorter scan. The body ends at the next top-level
    `def` or `class`.

    The whole body is returned as one unit rather than line by line, and that is
    the reason this file has its own reader for Python. A coverage command there
    is a list literal spanning several lines with no continuation to fold, so read
    a line at a time the `cargo llvm-cov` that introduces the flags arrives on one
    line and the flags arrive on the next, and neither half reads as a command.

    A body the tokenizer rejects is returned as it stands, which is the safe
    direction here: a command the reader cannot clean up is still worth reading,
    because a missed command is a missed finding. `strip_python_prose` returns
    None rather than the text for the opposite reason, and this caller is the one
    that can act on it.
    """
    lines = text.splitlines()
    opening = re.compile(rf"^def\s+{re.escape(name)}\s*\(")
    start = next((i for i, raw in enumerate(lines) if opening.match(raw)), None)
    if start is None:
        return None
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if re.match(r"^(def|class)\s", lines[index]):
            end = index
            break
    body = "\n".join(lines[start:end])
    stripped = strip_python_prose(body)
    return start + 1, body if stripped is None else stripped


# --- rules --------------------------------------------------------------------


def _object_problem(invocation: Invocation) -> str | None:
    """Why this run's merge can see an object the run did not build, or None."""
    return (
        "a coverage run naming fewer packages than the workspace, with nothing "
        "in this file to keep the merge inside the packages it selected: "
        f"`{invocation.render()}`. The report's object list is built by "
        "walking the instrumented target directory and a package selector does "
        "not narrow that walk, so an earlier run's binaries change this file's "
        "line set and function count. Either delete the foreign objects before "
        "the report step or give the run an instrumented directory of its own."
    )


def _boundary_problem(invocation: Invocation) -> str | None:
    """Why this run has no boundary to act in, or None."""
    return (
        "a coverage run that names fewer packages than the workspace and both "
        "runs the tests and writes the report in one command: "
        f"`{invocation.render()}`. The gate can only drop an unreadable "
        "profile buffer or an instrumented binary the run did not build in the "
        "gap between a test step and a report step, and one command has no gap. "
        "`--no-report` on the test step is what leaves the buffers on disk for it."
    )


def _report_problem(invocation: Invocation) -> str | None:
    """Why this report step's merge can be ended by one bad buffer, or None."""
    mode = failure_mode(invocation.tokens)
    if mode == "all":
        return None
    if mode in (None, ""):
        lead = (
            "a coverage report step passing no failure mode: "
            f"`{invocation.render()}`"
        )
    else:
        lead = (
            f"a coverage report step passing `--failure-mode {mode}`, which is the "
            "tool's default and behaves exactly as if the flag were absent: "
            f"`{invocation.render()}`"
        )
    return (
        f"{lead}. The default fails the merge as soon as any one profile buffer "
        "cannot be read, so a single malformed buffer ends the whole merge after "
        "every test has already passed. `--failure-mode=all` fails only when every "
        "buffer is unreadable, so one buffer loses its own coverage instead."
    )
