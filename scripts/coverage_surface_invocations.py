"""Finding the coverage commands in a text and saying what each one does."""

from __future__ import annotations

from typing import NamedTuple

from coverage_surface_text import (
    _VARIABLE,
    _is_cargo,
    _logical_lines,
    _protect_variables,
    _tokens,
    expand,
)

#: The tool's own subcommands, read out of its embedded help rather than guessed,
#: because the subcommand is what says whether a command runs tests or only reads
#: the buffers they wrote. `run` is in the list and not in the test set: it runs a
#: binary, and the coverage it produces is not a test's.
SUBCOMMANDS = frozenset({"test", "run", "report", "show-env", "clean", "nextest"})

#: The subcommands that run the test suite, plus the absence of one, which is the
#: tool's own default and does run the tests.
TEST_SUBCOMMANDS = frozenset({"nextest", "test"})

#: The flags that make an invocation write a report. `--summary-only` is here
#: because it can only be used together with one of the export formats, and
#: `--no-run` because the tool documents it as generating the report without
#: running the tests, which is a report written by a command meant to test.
REPORT_FLAGS = frozenset(
    {
        "--json",
        "--lcov",
        "--cobertura",
        "--codecov",
        "--text",
        "--html",
        "--open",
        "--summary-only",
        "--output-path",
        "--output-dir",
        "--no-run",
    }
)


def subcommand_of(tokens: list[str]) -> str | None:
    """The coverage tool's subcommand on this command line, or None.

    Matched against the tool's own list rather than read as "the first word after
    `llvm-cov` that is not a flag". That shorter rule read the package name in
    `cargo llvm-cov -p clio-store --json` as the subcommand, because a value-taking
    flag looks exactly like a flag followed by a bare word, so a per-package run
    with no subcommand read as something that runs no tests and rule 2 said
    nothing about it.

    `None` means no subcommand was named, which is the tool's default.
    """
    try:
        start = tokens.index("llvm-cov") + 1
    except ValueError:
        return None
    return next((token for token in tokens[start:] if token in SUBCOMMANDS), None)


def writes_report(tokens: list[str]) -> bool:
    """Whether this command line writes a coverage report."""
    if subcommand_of(tokens) == "report":
        return True
    return any(token in REPORT_FLAGS for token in tokens)


def runs_tests(tokens: list[str]) -> bool:
    """Whether this command line runs the test suite.

    True when no subcommand is named, because that is the tool's default: an
    unqualified `cargo llvm-cov` runs the tests and then writes the report, which
    is the shape rule 2 exists for.
    """
    subcommand = subcommand_of(tokens)
    return subcommand is None or subcommand in TEST_SUBCOMMANDS


def names_subset(tokens: list[str]) -> bool:
    """Whether this command line narrows the run below the workspace.

    A package selector is what narrows it, and a `--workspace` on the same line
    is what widens it back, so both are read. A command line naming neither
    selector narrows nothing and is not a subset.
    """
    if "--workspace" in tokens:
        return False
    return any(
        token in ("-p", "--package") or token.startswith("--package=") for token in tokens
    )


def failure_mode(tokens: list[str]) -> str | None:
    """The merge failure mode this command line asks for, or None.

    Read in both spellings, `--failure-mode all` and `--failure-mode=all`, for
    the same reason the package selector is read in both: a check that missed one
    spelling would report the value the command gives as the one it omits.
    """
    for index, token in enumerate(tokens):
        if token.startswith("--failure-mode="):
            return token.split("=", 1)[1]
        if token == "--failure-mode":
            return tokens[index + 1] if index + 1 < len(tokens) else ""
    return None


class Invocation(NamedTuple):
    """One coverage command, with the file and line it was read from."""

    path: str
    line: int
    tokens: list[str]

    def render(self) -> str:
        """The command as a reader would recognise it.

        A name this check could not resolve is written in make's braces rather
        than with the sentinel that stood in for it, because a message quoting
        control characters sends the reader to the wrong place.
        """
        return " ".join(_VARIABLE.sub(lambda m: "${" + m.group(1) + "}", token) for token in self.tokens)


def _cargo_index(tokens: list[str]) -> int | None:
    """Where the cargo executable sits on this command line, or None."""
    return next((i for i, token in enumerate(tokens) if _is_cargo(token)), None)


def coverage_invocations(text: str, selections: dict[str, list[list[str]]]) -> list[Invocation]:
    """Every command that runs the coverage tool through cargo.

    A cargo executable is required as well as the tool, which is what keeps a
    dispatch arm such as `test | nextest | llvm-cov | clippy) heavy=1` and an
    install line naming `cargo-llvm-cov` from reading as invocations: neither
    carries a bare `cargo`.

    Make variables are expanded first, because a coverage command written as
    `NAME = $(CARGO) llvm-cov …` and used as `$(NAME) --json …` is one command
    spread over two lines, and read one line at a time each half looks like
    something else.
    """
    found: list[Invocation] = []
    for number, joined in _logical_lines(text):
        tokens = _tokens(expand(_protect_variables(joined), selections))
        start = _cargo_index(tokens)
        if start is None or "llvm-cov" not in tokens[start + 1 :]:
            continue
        found.append(Invocation("", number, tokens[start:]))
    return found
