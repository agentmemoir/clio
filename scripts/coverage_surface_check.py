#!/usr/bin/env python3
"""Checks that a coverage run still measures one thing and reports it once.

The coverage surface carries four protections, and nothing in the tree stops a
later change from removing any of them:

- the instrumented run is split into a step that runs the tests and a step that
  writes the report, so the gate owns a boundary between them;
- a profile buffer too short to hold a raw profile header is deleted before the
  merge, and named when it is;
- an instrumented binary the run did not build is deleted before the merge, so
  the report cannot union a foreign object's rows into a file's numbers;
- a per-package measurement builds into an instrumented directory of its own.

This repository has already lost a make target's checks while keeping its name,
once, and `scripts/lint_scope_check.py` is the check whose whole purpose is to
catch that class. This file is the same kind of check for the coverage surface.
Every rule below is a configuration rule: it reads what the commands and the
commands' surroundings say, and it refuses a shape that has quietly stopped
meaning what it says. It runs no build and no coverage command.

Rules, each read where the thing it describes lives:

1. A coverage invocation that names fewer packages than the workspace, and takes
   its report from the shared instrumented directory, without either pinning an
   instrumented directory of its own or restricting the objects the merge may
   see, is a finding. A `--workspace` run is exempt, because it rebuilds every
   package and so holds no foreign binary. This is measured, not preferred: the
   coverage tool builds a report's object list by walking the instrumented
   target directory, and a package selector does not narrow that walk. Either
   protection is read from the file with its comments and its docstrings removed,
   so a sentence naming one is not one: this check exists to survive a later edit,
   and the edit that removes a protection is very often the sentence explaining
   that the protection was removed.
2. A coverage invocation that names fewer packages than the workspace and both
   runs the tests and writes the report in one command is a finding, and a run
   with no separate report step at all is the same finding, because the boundary
   this rule protects is the two steps and it is the only place the gate can drop
   a bad buffer or a foreign object. A report is written by a report flag or by
   `--no-run`, which the tool documents as generating a coverage report without
   running tests, so both count. A `--workspace` run is exempt from this rule and
   not from rule 3, for the reason in rule 1: what remains for it is the
   malformed buffer, and the failure mode is what answers that.
3. A coverage report step that does not ask the merge to fail only when every
   buffer is unreadable is a finding. That means no `--failure-mode` at all, and
   also `--failure-mode any`, which is the tool's default and behaves exactly as
   if the flag were absent: one malformed buffer ends the whole merge after every
   test has passed.
4. A function that writes a coverage report must drop the profile buffers it
   cannot read and delete the objects the run did not build, and both must happen
   before it writes the report, because the merge is what consumes them. This is
   the one rule here that is not about a command line, and it is here because the
   other three cannot see it: rule 1 is satisfied when a file names either
   protection, so deleting one of the two calls leaves the other to answer for it
   and the file still reads as protected.
5. A file this check must read and cannot read is never a pass, and neither is a
   file whose comments this reader cannot tell from its code. A scan that
   declines to look cannot report what it did not see, and a stripper that
   guessed would cut a command in half and hide the very invocation this check
   exists to report.

A file this check cannot read is not a skip, and neither is a builder it cannot
find. Both are reported: a missing file or a missing command builder is a finding
rather than a pass, because a Makefile that had been renamed, or a split that had
been undone, would otherwise turn this into a check of nothing.

What is not scanned, stated rather than left to be assumed:

- **Python other than `scripts/scoped_runner.py`.** The gate's own coverage
  commands are only ever expressed as Python list literals, so a check that
  ignored Python would find nothing to protect and this file would be the third
  copy of the same false negative. What is read is scoped to the two named
  command builders rather than grepped, because a grep over a Python file reads
  this file's own docstring and its own fixtures as coverage invocations, and the
  false positives cost more than the hole.
- **A protection marker is looked for in the whole file, not in the run.** A file
  that pins an instrumented directory of its own for one run and leaves another
  run in the same file unprotected is a false pass for rule 1. Telling them apart
  needs the run to be executed, and this check runs nothing. Rule 4 does not have
  that limit, because it reads the calls rather than the file.
- **A comment is not a protection, and neither is a docstring.** Comments and
  docstrings are removed before the markers are looked for, in the Makefile, in
  the shell scripts and in the workflows as well as in the runner, because a file
  that only mentions a protection reads as holding one. What is left out of the
  strip, and so still counts as a marker, is stated rather than assumed:
  - A `#` inside quotes, inside a `${NAME#prefix}` or `$(NAME)` expansion, after
    a `$`, or after a backslash is text, not a comment, and stays.
  - In a workflow a `#` starts a comment only at the start of a line or after a
    space, which is what YAML says, so `tag: v#1` keeps its value.
  - A line ending while a quote or an expansion is still open is kept whole. The
    quote began on an earlier line, and cutting at this line's `#` could drop a
    command that runs.
  - A `#` inside a backquote pair is a comment in the shell that reads it, so the
    line is cut there and nothing that runs is lost.
- **A file this reader cannot strip is refused, not judged.** Two things are
  refused: a file whose kind has no comment model here, and a file holding a
  here-document, whose body is data rather than commands and so has no `#` that
  starts a comment and no line that runs. Both are reported like a file that could
  not be read, and both fail closed. Neither is a silent skip: this repository
  holds no file of either kind today, so a refusal names a change to make.
- **A docstring written on the same line as its `def` is kept.** The reader takes
  a docstring to be the first statement of a block, and a string in a parameter
  list is a default value rather than prose, so the two cannot be told apart
  without parsing the signature. No file in this repository is written that way.
- **Rule 4 attributes a call to the function that writes it,** which means a
  hygiene step reached only through a helper called by two runs is seen once, at
  the helper, and the run that skipped it is not named. Reading a call graph
  needs a whole file, and this file reads two functions of one.
- **Rule 4 reads the stripped text, so a comment cannot answer for a hygiene step.**
  It used to read the raw line, which meant `pass  # prune_foreign_objects(...)`
  satisfied it: a run that had deleted both steps read as protected as long as
  someone left their names in a comment. Both readers now take the same stripped
  form, and the strip blanks prose instead of deleting it so a reported line number
  is still the line number in the source. A `#` inside a quoted string or a
  `${NAME#prefix}` expansion survives, because the strip tracks quoting and
  expansion rather than cutting at the first `#`.
- **Cargo aliases.** An alias defined in `.cargo/config.toml` or in the root
  manifest that expands to `llvm-cov` is invisible here, because nothing runs to
  expand it. A command line is read, not executed.
- **`scripts/gate.py`,** the one file that narrows a command on the direct-cargo
  path. It carries no coverage invocation to narrow: its coverage arm only names
  a kind. That is covered by the gate's own self-test rather than by this file.
- **Whether the protections work.** This reads configuration. It cannot show that
  the buffer scan finds every bad buffer, that the object rule keeps what the run
  needs, or that a report written this way is correct. The first two are the
  gate's own self-test cases; the third needs a real run, which this file refuses
  to perform because an instrumented build is expensive and shared.

Usage:
  python3 scripts/coverage_surface_check.py
  python3 scripts/coverage_surface_check.py --self-test
  python3 scripts/coverage_surface_check.py --list-scanned

Exit codes:
  0  every coverage invocation is a shape that can still measure one run and
     report it once, and both command builders are present
  1  at least one finding, each naming the file and line
  2  usage error
  3  a file or directory this check had to read could not be read, so the clean
     parts of the run do not cover it
"""

from __future__ import annotations

import contextlib
import io
import re
import sys
import tempfile
import tokenize
from pathlib import Path
from typing import NamedTuple

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

USAGE = "usage: coverage_surface_check.py | --self-test | --list-scanned"

#: The gate itself. Its two command builders are where the boundary this check
#: protects is written, and its two hygiene steps are where the boundary is used.
RUNNER = "scripts/scoped_runner.py"

#: Where the coverage commands are assembled for the make targets.
MAKEFILE = "Makefile"

#: Files that must exist for this check to have read the whole surface. A missing
#: one is a finding rather than a pass, for the reason rule 4 gives.
REQUIRED = (RUNNER, MAKEFILE)

#: The two command builders. Named rather than grepped, so a Python file's prose
#: and its own fixtures cannot read as coverage commands.
TEST_BUILDER = "coverage_test_command"
REPORT_BUILDER = "coverage_report_command"

#: The two hygiene steps, and what removing each one costs. Rule 4.
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

#: The mark that says a file restricts the objects its report may merge, and the
#: key that says it pins an instrumented directory of its own. Either one is
#: enough, and both are here because they are the two ways this repository has
#: answered the foreign-object problem.
PRUNE_MARKER = "prune_foreign_objects("
DIRECTORY_KEY = "CARGO_LLVM_COV_TARGET_DIR"

#: The cargo executables reached by name. A protected `$(CARGO)` arrives carrying
#: its own name, so the comparison folds case: a make variable is upper case and
#: cargo is lower.
CARGO_EXECUTABLES = frozenset({"cargo", "cargo-llvm-cov"})

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

#: Directories searched for shell scripts, each non-recursively, and the shell
#: files that carry no extension. The same bounded list the lint scope check
#: uses: a walk from the repository root would descend into the build directory,
#: where there are thousands of files and none of them are commands.
SHELL_DIRS = ("scripts", "hooks.d")

SHELL_EXTRA = (
    "scripts/gate-shim",
    "scripts/git-hooks/pre-commit",
    "scripts/git-hooks/pre-merge-commit",
)

#: A make variable reference, in either spelling, matched before it is replaced.
#: Both spellings are matched because a check that resolved `$(CARGO)` and left
#: `${CARGO}` as a literal word would read a command that reaches cargo through one
#: spelling as one that reaches nothing.
#:
#: A reference is replaced rather than parsed because the tokenizer has to strip
#: brackets off the edges of every token to read a Python command literal, and
#: `$(CARGO)` would arrive as `$(CARGO`. The sentinel is a byte no command line
#: contains and no edge-punctuation set holds, so it survives the strip.
_PROTECT = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$\(([A-Za-z_][A-Za-z0-9_]*)\)")

_VARIABLE = re.compile("\x00([A-Za-z_][A-Za-z0-9_]*)\x00")

#: `NAME = value` in any assignment form make honours, including a target-specific
#: one and the `export ` form of the same thing.
ASSIGNMENT = re.compile(
    r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*(?:::=|:\??=|\?=|=)\s*(.*)$"
)

#: Punctuation stripped from the edges of a token. A coverage command written as
#: a Python list literal is full of it: `"cargo",` and `("-p", name)` are one
#: token each once this is gone, and without it the flag every rule reads would
#: arrive as `"--no-report",` and match nothing.
EDGE_PUNCTUATION = "\"'`()[]{},:*;"

#: The comment model of each kind of file this check reads. A kind that is not
#: here is refused rather than read under one of these, because a `#` starts a
#: comment in all three and is an ordinary character in some other languages, and
#: a file read as one of the three would have its code cut away.
KIND_MAKE = "make"
KIND_SHELL = "shell"
KIND_YAML = "yaml"

#: The suffixes that name a kind on their own.
KIND_BY_SUFFIX = {".sh": KIND_SHELL, ".yml": KIND_YAML, ".yaml": KIND_YAML}

#: The interpreters a shebang may name for an extension-less file to be shell.
#: The repository lists its own extension-less scripts by name, so the shebang is
#: the second answer and the suffix the first, never the other way round.
SHELL_INTERPRETERS = frozenset({"sh", "bash", "dash", "ksh", "zsh"})

#: A here-document. Its body is data and not commands, so a `#` in it starts no
#: comment and a command-shaped line in it runs nowhere: a reader that cannot tell
#: the body from the script cannot judge either, and the file is refused. The
#: YAML merge key is spelled the same two characters and is not one, so it is
#: excluded rather than mistaken for the start of a body.
HEREDOC = re.compile(r"<<(?!:)-?\s*[\"'A-Za-z_]")

#: Why a file was refused, as the sentence a reader gets with the path.
REFUSALS = {
    "kind": "this reader has no comment model for a file of this kind, and "
    "reading it as one of the kinds it does model could cut a command in half",
    "heredoc": "this file holds a here-document, whose body is data rather than "
    "commands, so a `#` in it starts no comment and a command-shaped line in it "
    "runs nowhere",
}


def _protect_variables(text: str) -> str:
    """Replace every `$(NAME)` and `${NAME}` with a sentinel-wrapped NAME."""
    return _PROTECT.sub(lambda m: f"\x00{m.group(1) or m.group(2)}\x00", text)


def _is_cargo(token: str) -> bool:
    """Whether a token names the coverage tool's cargo executable.

    A make variable is read through its own name rather than through its text, so
    `CARGO ?= cargo` passes and `$(RUSTUP_BIN)/cargo` does not: the comparison is
    against the executable, not against a spelling of it.
    """
    named = _VARIABLE.fullmatch(token)
    bare = named.group(1) if named is not None else token.strip(EDGE_PUNCTUATION)
    return bare.rsplit("/", 1)[-1].lower() in CARGO_EXECUTABLES


def comment_kind(name: str, text: str) -> str | None:
    """The comment model a file is read under, or None when there is none.

    Read from the name first and the shebang second, because an extension-less
    script has no suffix to read and its own first line is what says what it is.
    None means this reader has no rule for the file and the caller refuses it.
    """
    if name == MAKEFILE or Path(name).name == MAKEFILE:
        return KIND_MAKE
    suffix = Path(name).suffix
    if suffix in KIND_BY_SUFFIX:
        return KIND_BY_SUFFIX[suffix]
    first = text.split("\n", 1)[0]
    words = first[2:].split() if first.startswith("#!") else []
    if words and Path(words[0]).name in SHELL_INTERPRETERS:
        return KIND_SHELL
    return None


def _strip_line(line: str, kind: str) -> str:
    """The line without its comment, or the whole line when there is none to cut.

    One rule per kind this reader models. In a Makefile and a shell script a `#`
    outside quotes starts a comment anywhere on the line. In a workflow it starts
    one only at the start of a line or after a space, which is what YAML says, so
    `tag: v#1` keeps its value and a trailing `# note` goes.

    A `#` inside quotes, inside a `${NAME#prefix}` or `$(NAME)` expansion, after a
    `$`, or after a backslash is text and never a comment, so none of the three
    can cut a command in half. A line that ends while a quote or an expansion is
    still open keeps its `#`: the quote was opened on an earlier line and this
    line may be its middle, where a `#` is a character and not the start of
    anything.
    """
    quote = None
    depth = 0
    index = 0
    while index < len(line):
        char = line[index]
        if quote is not None:
            # A backslash escapes the closing quote of a double-quoted string but
            # is an ordinary character inside a single-quoted one, which is the
            # shell's own rule and the one that keeps `'\''` whole.
            if char == quote and not (quote == '"' and line[index - 1 : index] == "\\"):
                quote = None
        elif char == "\\":
            index += 1
        elif char in "'\"":
            quote = char
        elif line.startswith("${", index) or line.startswith("$(", index):
            depth += 1
            index += 1
        elif depth and char in ")}":
            depth -= 1
        elif char == "#" and not depth and line[index - 1 : index] != "$":
            if kind == KIND_YAML and index and not line[index - 1].isspace():
                index += 1
                continue
            return line[:index]
        index += 1
    return line


def strip_comments(text: str, kind: str) -> str | None:
    """`text` with its comments removed, one output line per input line, or None.

    Lines keep their positions, so a finding still names the line a reader sees in
    the file itself. None means the file holds something this reader does not
    model and the caller refuses it rather than judging it: the one construct
    looked for is a here-document, whose body this reader has no rule for.

    One file gets one comment model, and it is chosen here. Both the markers and
    the command lines are read from what this returns, so a line the stripper cut
    can neither hide a protection nor hide an invocation.
    """
    if HEREDOC.search(text):
        return None
    stripped = "\n".join(_strip_line(line, kind) for line in text.splitlines())
    # Put back the newline a final empty line is written as, so the result is a
    # line-for-line copy of the file and a finding's line is the file's line.
    return stripped + "\n" if text.endswith("\n") else stripped


def _logical_lines(text: str):
    """Yield (first line number, joined command), folding `\\` continuations.

    Make wraps a long recipe across lines. Read one line at a time, the second
    physical line of a wrapped recipe looked like a command that named no
    packages, which would make a wrapped command read as a different one.

    `text` is expected to have its comments removed already, by `strip_comments`
    or by `strip_python_prose`, because a comment cannot be folded into a command
    and one file has one comment model.
    """
    buffer = ""
    start = 0
    for number, line in enumerate(text.splitlines(), start=1):
        if not buffer:
            start = number
        body = line.rstrip()
        if body.endswith("\\"):
            buffer += body[:-1] + " "
            continue
        buffer += line
        if buffer.strip():
            yield start, buffer
        buffer = ""
    if buffer.strip():
        yield start, buffer


def _tokens(text: str) -> list[str]:
    """The words of a command, with the punctuation around them removed."""
    words = []
    for raw in text.split():
        word = raw.strip(EDGE_PUNCTUATION)
        if word:
            words.append(word)
    return words


def variable_selections(text: str) -> dict[str, list[list[str]]]:
    """Every `NAME = ...` in a makefile, as {name: [values, values...]}.

    All the definitions are kept, and the last one is what a command reads, because
    make takes the last assignment before use and a Makefile that sets a name
    twice runs the second one. A reference with no definition is left alone rather
    than assumed empty, so it cannot be mistaken for a command that named nothing.
    """
    found: dict[str, list[list[str]]] = {}
    for _, joined in _logical_lines(text):
        for token_line in joined.split(";"):
            match = ASSIGNMENT.match(token_line.strip())
            if match is None:
                continue
            found.setdefault(match.group(1), []).append(match.group(2).split())
    return found


def expand(text: str, selections: dict[str, list[list[str]]], depth: int = 4) -> str:
    """Substitute every make variable reference with its last definition.

    The input is expected to be protected already, and a substituted value is
    protected again, so a definition that references another name resolves in the
    next pass. The pass count is bounded rather than open because a definition
    that references itself would otherwise loop.

    A name with no definition is left in place, so an unknown variable cannot
    shorten a command into one that reads as narrower than it is. That is the
    conservative direction: a command this check cannot resolve is not a finding.
    """
    for _ in range(depth):
        replaced = False

        def substitute(match: re.Match[str]) -> str:
            nonlocal replaced
            values = selections.get(match.group(1))
            if not values:
                return match.group(0)
            replaced = True
            return _protect_variables(" ".join(values[-1]))

        text = _VARIABLE.sub(substitute, text)
        if not replaced:
            break
    return text


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
            last = end_col if row == end_row else len(chars)
            for index in range(line + first, min(line + last, len(chars))):
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

    A function that runs the tests and writes the report is a run, and each run is
    asked whether it drops the buffers it cannot read and deletes the objects the
    run did not build, and whether it does both before the report rather than
    after. A function that does only one of the two jobs is not asked: the report
    step merges what its caller already cleaned, and a test that names the
    commands in a string is not a run.
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


class Finding:
    """One thing that is wrong, and where it was found."""

    def __init__(self, path: str, line: int, rule: str, message: str) -> None:
        self.path = path
        self.line = line
        self.rule = rule
        self.message = message

    def __str__(self) -> str:
        where = f"{self.path}:{self.line}" if self.line else self.path
        return f"{where}: {self.rule}: {self.message}"


class Surface:
    """The files this check has to read, and the verdict on each."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.findings: list[Finding] = []
        self.unreadable: list[str] = []
        self.scanned: list[str] = []
        self.invocations = 0

    def exit_code(self) -> int:
        """The verdict. A path this check could not read outranks a finding.

        The unreadable case comes first on purpose: a clean run over the files it
        did reach is not a clean run, and reporting it as one is the failure this
        check was written after.
        """
        if self.unreadable:
            return EXIT_UNREADABLE
        return EXIT_FINDINGS if self.findings else EXIT_OK

    def read(self, relative: str) -> str | None:
        path = self.root / relative
        self.scanned.append(relative)
        if not path.exists():
            # A missing file is already a finding from REQUIRED. Recording it as
            # unreadable as well would report one absence twice and give the run
            # the wrong exit code.
            return None
        try:
            return path.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            self.unreadable.append(f"{relative}: {error.strerror or error}")
            return None

    def code(self, relative: str, text: str) -> str | None:
        """`text` with its comments removed, or None when it is refused.

        A refusal is recorded against the unreadable paths rather than passed over,
        for the reason rule 5 gives: this reader either knows a file's comments or
        it does not judge the file, and either way it must not report a clean run
        over the part it skipped.
        """
        kind = comment_kind(relative, text)
        if kind is None:
            self.unreadable.append(f"{relative}: {REFUSALS['kind']}")
            return None
        stripped = strip_comments(text, kind)
        if stripped is None:
            self.unreadable.append(f"{relative}: {REFUSALS['heredoc']}")
            return None
        return stripped

    def check(self, relative: str, code: str) -> None:
        """Apply every rule to one file's coverage invocations.

        `code` is the file with its comments removed, so a marker found in it is
        on a line that does something. The markers are still read from the whole
        file rather than from the run, and the limit that costs is stated in the
        module docstring: a file that pins a directory for one run and leaves
        another unprotected passes here.
        """
        protected = PRUNE_MARKER in code or DIRECTORY_KEY in code
        for invocation in coverage_invocations(code, variable_selections(code)):
            self.invocations += 1
            tokens = invocation.tokens
            subset = names_subset(tokens)
            if subset and not protected:
                self.findings.append(
                    Finding(
                        relative,
                        invocation.line,
                        "object-restriction",
                        _object_problem(invocation),
                    )
                )
            if subset and runs_tests(tokens) and writes_report(tokens):
                self.findings.append(
                    Finding(relative, invocation.line, "boundary", _boundary_problem(invocation))
                )
            problem = _report_problem(invocation)
            if writes_report(tokens) and problem is not None:
                self.findings.append(
                    Finding(relative, invocation.line, "failure-mode", problem)
                )

    def check_runner(self, text: str) -> None:
        """Read the gate's two command builders, which are Python literals.

        The whole-file rules cannot see them: a Python list literal has one token
        per line and no shell to fold, so a scan that ignored the language would
        find no coverage command here at all and this check would protect nothing
        that matters.

        The markers are read from the file with its comments and its docstrings
        removed, because a note explaining that the object deletion is gone is not
        the object deletion, and because the file that holds the command builders
        is the file most likely to be edited to explain a change to them. A file
        the tokenizer refuses is refused rather than judged, for the reason
        `strip_python_prose` gives. Rule 4 reads the same stripped text, and needs
        to: a trailing comment naming a hygiene step used to satisfy it, so a run
        that had deleted both steps still read as protected as long as someone
        left the names behind in a comment. The strip blanks prose rather than
        deleting it, so a line number this check reports is the line number in the
        source.
        """
        code = strip_python_prose(text)
        if code is None:
            self.unreadable.append(
                f"{RUNNER}: this reader cannot tell its comments from its code, so a "
                "comment naming a protection would answer for one"
            )
            return
        protected = PRUNE_MARKER in code or DIRECTORY_KEY in code
        for line, rule, message in _hygiene_problem(code):
            self.findings.append(Finding(RUNNER, line, rule, message))
        for name, is_test in ((TEST_BUILDER, True), (REPORT_BUILDER, False)):
            found = builder_body(text, name)
            if found is None:
                self.findings.append(
                    Finding(
                        RUNNER,
                        0,
                        "boundary",
                        f"`{name}` is not defined here, so the coverage run has no "
                        f"{'test step that leaves its buffers on disk' if is_test else 'separate report step'} "
                        "to own a boundary between. One command cannot both run the "
                        "tests and write the report, because the gate has to drop a "
                        "bad buffer and a foreign object in the gap.",
                    )
                )
                continue
            number, body = found
            tokens = _tokens(body)
            start = _cargo_index(tokens)
            if start is None or "llvm-cov" not in tokens[start + 1 :]:
                continue
            invocation = Invocation(RUNNER, number, tokens[start:])
            self.invocations += 1
            subset = names_subset(tokens)
            if subset and not protected:
                self.findings.append(
                    Finding(RUNNER, number, "object-restriction", _object_problem(invocation))
                )
            if subset and runs_tests(tokens) and writes_report(tokens):
                self.findings.append(
                    Finding(RUNNER, number, "boundary", _boundary_problem(invocation))
                )
            problem = _report_problem(invocation)
            if writes_report(tokens) and problem is not None:
                self.findings.append(Finding(RUNNER, number, "failure-mode", problem))


# --- scanning -----------------------------------------------------------------


def _shell_paths(root: Path) -> list[str]:
    """Every shell script in the bounded search, as a relative path."""
    paths: list[str] = []
    for directory in (".",) + SHELL_DIRS:
        try:
            found = sorted((root / directory).glob("*.sh"))
        except OSError:
            continue
        paths.extend(str(path.relative_to(root)) for path in found if path.is_file())
    paths.extend(name for name in SHELL_EXTRA if (root / name).is_file())
    return sorted(dict.fromkeys(paths))


def scan(root: Path) -> Surface:
    surface = Surface(root)

    for relative in REQUIRED:
        if not (root / relative).exists():
            surface.findings.append(
                Finding(relative, 0, "surface", "a file this check must read is missing")
            )

    runner = surface.read(RUNNER)
    if runner is not None:
        surface.check_runner(runner)

    makefile = surface.read(MAKEFILE)
    if makefile is not None:
        stripped = surface.code(MAKEFILE, makefile)
        if stripped is not None:
            surface.check(MAKEFILE, stripped)

    try:
        workflow_dir = root / ".github/workflows"
        workflows = sorted(workflow_dir.glob("*.yml")) + sorted(workflow_dir.glob("*.yaml"))
    except OSError as error:
        surface.unreadable.append(f".github/workflows: {error.strerror or error}")
        workflows = []
    if not workflows:
        surface.unreadable.append(".github/workflows: no workflow file could be listed")

    for workflow in workflows:
        relative = f".github/workflows/{workflow.name}"
        text = surface.read(relative)
        if text is not None:
            stripped = surface.code(relative, text)
            if stripped is not None:
                surface.check(relative, stripped)

    for relative in _shell_paths(root):
        text = surface.read(relative)
        if text is not None:
            stripped = surface.code(relative, text)
            if stripped is not None:
                surface.check(relative, stripped)

    return surface


def report(root: Path) -> int:
    surface = scan(root)
    for path in surface.unreadable:
        print(f"coverage-surface-check: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"coverage-surface-check: FINDING {finding}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        print(
            f"coverage-surface-check: {len(surface.unreadable)} path(s) this check had to read "
            "could not be read, so the rest of this run does not cover them",
            file=sys.stderr,
        )
        return code
    if code == EXIT_FINDINGS:
        print(f"coverage-surface-check: {len(surface.findings)} finding(s)")
        return code
    print(
        f"coverage-surface-check: every coverage invocation measures one run and reports "
        f"it once ({surface.invocations} invocation(s) over {len(surface.scanned)} file(s))"
    )
    return code


# --- Self-test -----------------------------------------------------------------
#
# Every case is a fixture tree, so the cases describe the check rather than the
# repository and the suite gives the same answer on any machine. The fixtures live
# under one temporary directory that is removed when the suite returns. Nothing
# here is executed, so no fixture has to run.

#: The gate's two command builders and its two hygiene markers, in the shape the
#: real file has. Written out rather than produced by editing a copy of it,
#: because substring surgery on a fixture is how a case ends up testing a shape it
#: never built.
CLEAN_RUNNER = '''#!/usr/bin/env python3
"""A fixture gate."""


def coverage_test_command(packages):
    command = [
        "cargo", "llvm-cov", "nextest",
        *[flag for name in packages for flag in ("-p", name)],
        "--locked", "--no-report",
    ]
    return command


def coverage_report_command(report_path):
    return [
        "cargo", "llvm-cov", "report",
        "--json", "--output-path", str(report_path),
        "--failure-mode=all",
    ]


def run_coverage_tests(packages, env):
    return command


def write_coverage_report(report_path, env):
    return 0


def drop_invalid_profraws(directory):
    return 0, 0


def prune_foreign_objects(directory, selected):
    return 0


def run_coverage(packages, report_path, env):
    run_coverage_tests(packages, env)
    directory = env["dir"]
    drop_invalid_profraws(directory)
    prune_foreign_objects(directory, packages)
    write_coverage_report(report_path, env)


TARGET_DIR_KEY = "CARGO_LLVM_COV_TARGET_DIR"
'''

#: The gate's own run with the object deletion removed. Rule 1 still passes,
#: because the file still pins an instrumented directory of its own for the
#: per-package mode, so this is exactly the removal the other three rules cannot
#: see.
NO_OBJECT_DELETE_RUNNER = CLEAN_RUNNER.replace(
    "    prune_foreign_objects(directory, packages)\n", ""
)

#: The same run with the buffer scan removed.
NO_BUFFER_DROP_RUNNER = CLEAN_RUNNER.replace("    drop_invalid_profraws(directory)\n", "")

#: The same run with the object deletion removed and its name left behind in a
#: trailing comment. A reader sees the step named in the file; the run does not
#: do it. This is the shape a note left behind by an earlier edit takes, and it is
#: the reason rule 4 reads the stripped text rather than the line.
COMMENTED_OBJECT_DELETE_RUNNER = CLEAN_RUNNER.replace(
    "    prune_foreign_objects(directory, packages)\n",
    "    pass  # prune_foreign_objects(directory, packages)\n",
)

#: The same run, deleting the objects for real and also naming the step in a
#: trailing comment. Nothing here is wrong, so it must not become a finding: a
#: reader that strips too much would call this one, and a reader that strips
#: nothing would have passed the case above.
COMMENTED_AND_CALLED_RUNNER = CLEAN_RUNNER.replace(
    "    prune_foreign_objects(directory, packages)\n",
    "    prune_foreign_objects(directory, packages)  # and the report step needs it\n",
)

#: The same run with both steps moved below the report, which is a reordering
#: rather than a deletion and is just as invisible to a reader looking for calls.
HYGIENE_AFTER_REPORT_RUNNER = CLEAN_RUNNER.replace(
    "    drop_invalid_profraws(directory)\n"
    "    prune_foreign_objects(directory, packages)\n"
    "    write_coverage_report(report_path, env)\n",
    "    write_coverage_report(report_path, env)\n"
    "    drop_invalid_profraws(directory)\n"
    "    prune_foreign_objects(directory, packages)\n",
)

#: The whole-workspace diagnostic. `--workspace` makes rule 1 and rule 2 exempt by
#: measurement, so the failure mode is the whole of what rule 3 asks of it, and it
#: carries one.
CLEAN_MAKEFILE = """LLVM_COV_CMD := $(CARGO) llvm-cov nextest --workspace --locked --no-clean \\
\t--profile ci --failure-mode=all

coverage cov:
\t$(LLVM_COV_CMD) --json --output-path coverage.json
"""

#: A workflow that installs the tool and calls the make target. Neither is a
#: coverage command line, and a check that read them as one would report every
#: workflow that names the tool at all.
CLEAN_WORKFLOW = """on: [pull_request]
jobs:
  check:
    steps:
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-llvm-cov
      - name: Scoped coverage
        run: make coverage-fast BASE=origin/master
"""

#: The test step with a docstring that talks about the tool's `report`
#: subcommand, in prose, before the command it documents. Read without stripping
#: the prose, the word `report` is a subcommand the check finds and the step reads
#: as a report writer that passes no failure mode — two findings against the one
#: function that is exactly right.
PROSE_RUNNER = CLEAN_RUNNER.replace(
    "def coverage_test_command(packages):\n    command = [",
    "def coverage_test_command(packages):\n"
    '    """Build and run the tests, writing no report.\n\n'
    "    The tool documents `--no-run` as the way to generate a report\n"
    "    without running tests, so both that flag and the report subcommand\n"
    "    are named here and neither is passed to it.\n"
    '    """\n'
    "    command = [",
)

CLEAN_FILES = {
    RUNNER: CLEAN_RUNNER,
    MAKEFILE: CLEAN_MAKEFILE,
    ".github/workflows/ci.yml": CLEAN_WORKFLOW,
}

#: The test step with the tool's deprecated `--no-run`, which the tool documents as
#: generating a report without running the tests. It carries a failure mode, so
#: rule 3 has nothing to say and the case proves rule 2 alone.
NO_RUN_RUNNER = CLEAN_RUNNER.replace(
    '"--locked", "--no-report",', '"--locked", "--no-run", "--failure-mode=all",'
)

#: The test step writing the report itself. A separate report builder is still
#: present, so this fixture produces one finding and not two.
ONE_COMMAND_RUNNER = CLEAN_RUNNER.replace(
    '"--locked", "--no-report",',
    '"--locked", "--json", "--output-path", "coverage.json", "--failure-mode=all",',
)

#: The report step with no failure mode at all.
NO_MODE_RUNNER = CLEAN_RUNNER.replace('"--failure-mode=all",\n', "")

#: The report step asking for the tool's default, which behaves as if the flag
#: were absent.
ANY_MODE_RUNNER = CLEAN_RUNNER.replace('"--failure-mode=all",', '"--failure-mode=any",')

#: The failure mode in the other spelling, so a check that read only one of them
#: would report a command that does pass one as one that does not.
SPACED_MODE_RUNNER = CLEAN_RUNNER.replace(
    '"--failure-mode=all",', '"--failure-mode", "all",'
)

#: One command in place of the two steps, which is the shape rule 2 exists for.
ONE_BUILDER_RUNNER = '''#!/usr/bin/env python3
"""A fixture gate."""


def coverage_test_command(packages):
    command = [
        "cargo", "llvm-cov", "nextest",
        *[flag for name in packages for flag in ("-p", name)],
        "--locked", "--json", "--output-path", "coverage.json",
        "--failure-mode=all",
    ]
    return command


def prune_foreign_objects(directory, selected):
    return 0
'''

def _without_report_builder(text: str) -> str:
    """The fixture with its report step removed and nothing else changed.

    Cut out by name rather than by truncating at the point where the report step
    starts, because a truncation also takes the hygiene functions with it and the
    test step then looks unprotected. That produces a second finding, and the case
    would pass with the wrong rule firing.
    """
    start = text.index(f"def {REPORT_BUILDER}")
    return text[:start] + text[text.index("def drop_invalid_profraws") :]


#: A per-package *report* step in a protected file. It names a package and it
#: writes a report, but it runs no tests, so rule 2 must not fire on it: a check
#: that read every command naming `-p` and a report flag as one command doing both
#: would fire here, and a later change adding a package filter to the report step
#: would be refused for a boundary it never crossed.
SUBSET_REPORT_MAKEFILE = """CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store

coverage cov:
\tcargo llvm-cov report -p clio-store --json --output-path coverage.json --failure-mode=all
"""

#: The same, with no subcommand named. That is the tool's default, and its
#: documented default runs the tests before writing the report, so this line runs
#: tests and writes a report in one command. A check that looked for the words
#: `nextest` or `test` and found neither would read it as a report step alone and
#: say nothing.
NO_SUBCOMMAND_BOUNDARY = """CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store

coverage cov:
\tcargo llvm-cov -p clio-store --json --output-path coverage.json --failure-mode=all
"""

#: A line naming both selectors. `--workspace` is read first, because a package
#: selector beside it widens the run back to every package rather than narrowing
#: it, and refusing that line would refuse a workspace-wide run for carrying a
#: selector it does not act on.
WORKSPACE_AND_PACKAGE = """COV_CMD := $(CARGO) llvm-cov nextest --workspace -p clio-store --locked --no-report

coverage cov:
\t$(COV_CMD)
"""

#: A per-package command in the Makefile with nothing to keep the merge inside the
#: packages it selected.
UNPROTECTED_MAKEFILE = """coverage cov:
\tcargo llvm-cov nextest -p clio-store --locked --no-report --failure-mode=all
"""

#: The same command in a Makefile that pins an instrumented directory of its own,
#: which is the other way this repository has answered the foreign-object problem.
PINNED_MAKEFILE = """CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store
COV_CMD := $(CARGO) llvm-cov nextest -p clio-store --locked --no-report

coverage cov:
\t$(CARGO) llvm-cov report --json --output-path coverage.json --failure-mode=all
"""

#: A whole-workspace command doing both in one invocation. Exempt from rule 2 by
#: measurement and by the reason rule 1 gives, and so a case rather than a hole.
WORKSPACE_ONE_COMMAND = """COV_CMD := $(CARGO) llvm-cov nextest --workspace --locked

coverage cov:
\t$(COV_CMD) --json --output-path coverage.json --failure-mode=all
"""

#: The two steps of a per-package run, as the Makefile writes them. Used on its own
#: by the comment case below, and by the shell and workflow cases that wrap it.
SUBSET_STEPS = (
    "COV_CMD := $(CARGO) llvm-cov nextest -p clio-store --locked --no-report\n"
    "\n"
    "coverage cov:\n"
    "\t$(COV_CMD)\n"
    "\t$(CARGO) llvm-cov report --json --output-path coverage.json --failure-mode=all\n"
)

#: A marker that only a comment carries. The comment is the sentence a person
#: writes while removing a protection, and it names the variable the removal
#: dropped, so read as a marker it silences the very rule that would report the
#: removal. Before the fix this file was judged protected and exited clean.
COMMENTED_MARKER_MAKEFILE = (
    "# the old approach set CARGO_LLVM_COV_TARGET_DIR by hand\n" + SUBSET_STEPS
)

#: The same run with the marker on a line that pins it, and a trailing comment on
#: that line, so a stripper that cut too eagerly fails here instead of passing.
PINNED_WITH_COMMENT_MAKEFILE = (
    "CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store"
    "  # one directory per package, not shared\n" + SUBSET_STEPS
)

#: A marker carried by a comment in a shell script rather than a Makefile.
COMMENTED_MARKER_SCRIPT = (
    "#!/bin/sh\n"
    "# an older script set CARGO_LLVM_COV_TARGET_DIR itself\n"
    "cargo llvm-cov nextest -p clio-store --locked --no-report\n"
    "cargo llvm-cov report --json --output-path coverage.json --failure-mode=all\n"
)

#: A marker carried by a comment in a workflow. The `#` sits after a space, so it
#: starts a comment by YAML's own rule and a stripper that cut at every `#` would
#: find the same thing.
COMMENTED_MARKER_WORKFLOW = (
    CLEAN_WORKFLOW
    + "      # this step used to export CARGO_LLVM_COV_TARGET_DIR\n"
    + "      - name: One crate\n"
    + "        run: cargo llvm-cov nextest -p clio-store --locked --no-report\n"
)

#: A `#` that is not a comment, with the command after it on the same line. The
#: `${NAME#prefix}` expansion strips a prefix from a value, so the character is
#: part of the name; the quoted `#` is part of a value. A stripper that cut at
#: either one would truncate the line, and a truncated line reads as no command at
#: all, which is a run this check cannot report.
EXPANSION_HASH_MAKEFILE = (
    "coverage cov:\n"
    "\tOUT=$${OUT#-} ; cargo llvm-cov nextest -p clio-store --locked --no-report\n"
)

QUOTED_HASH_MAKEFILE = (
    'coverage cov:\n'
    '\tOUT="dir#one" ; cargo llvm-cov nextest -p clio-store --locked --no-report\n'
)

#: A here-document, whose body is data rather than commands. No `#` in it starts a
#: comment and no line in it runs, so a reader that cannot tell the body from the
#: script must refuse the file rather than judge it.
HEREDOC_MAKEFILE = """coverage cov:
\tprintf '%s\\n' "$(cat <<'NOTES'
CARGO_LLVM_COV_TARGET_DIR is set per package, not by hand
NOTES
)"
\tcargo llvm-cov nextest -p clio-store --locked --no-report
"""


def _fixture(base: Path, label: str, changes: dict[str, str | None] | None = None) -> Path:
    """One case's tree: the clean files, with `changes` applied over them.

    The overrides are a dict of real relative paths rather than keyword arguments,
    because a keyword cannot carry the slash in `scripts/scoped_runner.py`.
    """
    files = dict(CLEAN_FILES)
    for relative, body in (changes or {}).items():
        if body is None:
            files.pop(relative, None)
        else:
            files[relative] = body
    root = base / label
    for relative, text in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return root


def _cases(base: Path) -> list[tuple[str, int, Path, list[str]]]:
    """(name, expected exit, fixture root, rules it must prove) per case.

    The rules are named because an exit code cannot say which rule fired. A case
    that proved its rule only by producing a non-zero exit could pass with a
    different rule firing instead, which is how a suite ends up green with the
    thing it protects deleted. A list rather than a set, so a case can assert that
    one rule fired twice. An empty list means the case expects no finding at all,
    which is the assertion a clean case needs.
    """
    wf = ".github/workflows/ci.yml"
    return [
        ("a clean tree passes", EXIT_OK, _fixture(base, "clean"), []),
        (
            "a docstring naming the report subcommand is prose, not the command",
            EXIT_OK,
            _fixture(base, "prose", {RUNNER: PROSE_RUNNER}),
            [],
        ),
        # Rule 3, the failure mode.
        (
            "a report step with no failure mode is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-mode", {RUNNER: NO_MODE_RUNNER}),
            ["failure-mode"],
        ),
        (
            "a report step asking for the default failure mode is a finding",
            EXIT_FINDINGS,
            _fixture(base, "any-mode", {RUNNER: ANY_MODE_RUNNER}),
            ["failure-mode"],
        ),
        (
            "the failure mode in its other spelling is accepted",
            EXIT_OK,
            _fixture(base, "spaced-mode", {RUNNER: SPACED_MODE_RUNNER}),
            [],
        ),
        (
            "a whole-workspace report step with no failure mode is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "makefile-no-mode",
                {MAKEFILE: CLEAN_MAKEFILE.replace(" --failure-mode=all", "")},
            ),
            ["failure-mode"],
        ),
        # Rule 2, the boundary. Each of these carries a failure mode, so rule 3
        # has nothing to say about them and the exit code cannot be reached by
        # another rule: the case proves the boundary or it proves nothing.
        (
            "a test step carrying --no-run is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-run", {RUNNER: NO_RUN_RUNNER}),
            ["boundary"],
        ),
        (
            "a test step writing the report itself is a finding",
            EXIT_FINDINGS,
            _fixture(base, "one-command", {RUNNER: ONE_COMMAND_RUNNER}),
            ["boundary"],
        ),
        (
            "one command in place of the two steps is a finding, twice over",
            EXIT_FINDINGS,
            _fixture(base, "one-builder", {RUNNER: ONE_BUILDER_RUNNER}),
            ["boundary", "boundary"],
        ),
        (
            "a whole-workspace run doing both in one command is exempt, and said so",
            EXIT_OK,
            _fixture(base, "workspace-one-command", {MAKEFILE: WORKSPACE_ONE_COMMAND}),
            [],
        ),
        # Rule 1, the object list.
        (
            "a per-package command nothing protects is a finding",
            EXIT_FINDINGS,
            _fixture(base, "unprotected", {MAKEFILE: UNPROTECTED_MAKEFILE}),
            ["object-restriction"],
        ),
        (
            "a per-package run in a makefile that pins its own directory is clean",
            EXIT_OK,
            _fixture(base, "pinned", {MAKEFILE: PINNED_MAKEFILE}),
            [],
        ),
        (
            "a per-package report step runs no tests, so it is not a boundary finding",
            EXIT_OK,
            _fixture(base, "subset-report", {MAKEFILE: SUBSET_REPORT_MAKEFILE}),
            [],
        ),
        (
            "a line naming --workspace beside a package selector is not a subset",
            EXIT_OK,
            _fixture(base, "workspace-and-package", {MAKEFILE: WORKSPACE_AND_PACKAGE}),
            [],
        ),
        (
            "a per-package command naming no subcommand still runs tests, by default",
            EXIT_FINDINGS,
            _fixture(base, "no-subcommand", {MAKEFILE: NO_SUBCOMMAND_BOUNDARY}),
            ["boundary"],
        ),
        (
            "a per-package command in a shell script is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "script-narrow",
                {"scripts/one.sh": "#!/bin/sh\ncargo llvm-cov nextest -p clio --locked\n"},
            ),
            ["object-restriction"],
        ),
        (
            "a whole-workspace command in a shell script is clean",
            EXIT_OK,
            _fixture(
                base,
                "script-whole",
                {
                    "scripts/one.sh": "#!/bin/sh\n"
                    "cargo llvm-cov nextest --workspace --locked --no-report\n"
                    "cargo llvm-cov report --json --failure-mode=all\n"
                },
            ),
            [],
        ),
        (
            "a per-package command in a workflow is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "workflow-narrow",
                {wf: CLEAN_WORKFLOW + "      - name: One crate\n        run: cargo llvm-cov nextest -p clio-store --locked\n"},
            ),
            ["object-restriction"],
        ),
        (
            "the shim carrying a dispatch arm is read and stays clean",
            EXIT_OK,
            _fixture(
                base,
                "shim",
                {
                    "scripts/gate-shim": "#!/bin/sh\n"
                    'case "$1" in\n  test | nextest | llvm-cov | clippy) heavy=1 ;;\nesac\n'
                },
            ),
            [],
        ),
        (
            "a commented coverage line is not an invocation",
            EXIT_OK,
            _fixture(base, "commented", {MAKEFILE: CLEAN_MAKEFILE + "# cargo llvm-cov nextest -p clio-store\n"}),
            [],
        ),
        # Rule 1 and prose. Each of these is the shape of change a person makes
        # while removing a protection: the sentence explaining it stays and names
        # the variable it removed, so a reader that counted words rather than
        # lines reported a clean run over a run that had lost one.
        (
            "a marker in a makefile comment does not protect a per-package run",
            EXIT_FINDINGS,
            _fixture(base, "commented-marker", {MAKEFILE: COMMENTED_MARKER_MAKEFILE}),
            # Twice, because the variable and the recipe that expands it are two
            # lines carrying one command. That is this check's existing reading and
            # the count is asserted so a case cannot pass with one run found twice
            # or a second run found at all.
            ["object-restriction", "object-restriction"],
        ),
        (
            "a marker in a shell script comment does not protect a per-package run",
            EXIT_FINDINGS,
            _fixture(base, "commented-marker-script", {"scripts/one.sh": COMMENTED_MARKER_SCRIPT}),
            ["object-restriction"],
        ),
        (
            "a marker in a workflow comment does not protect a per-package run",
            EXIT_FINDINGS,
            _fixture(base, "commented-marker-workflow", {wf: COMMENTED_MARKER_WORKFLOW}),
            ["object-restriction"],
        ),
        (
            "a marker a real line pins still protects, with a comment on that line",
            EXIT_OK,
            _fixture(base, "pinned-commented", {MAKEFILE: PINNED_WITH_COMMENT_MAKEFILE}),
            [],
        ),
        (
            "a # inside an expansion is not a comment and cuts no command off",
            EXIT_FINDINGS,
            _fixture(base, "expansion-hash", {MAKEFILE: EXPANSION_HASH_MAKEFILE}),
            ["object-restriction"],
        ),
        (
            "a # inside quotes is not a comment and cuts no command off",
            EXIT_FINDINGS,
            _fixture(base, "quoted-hash", {MAKEFILE: QUOTED_HASH_MAKEFILE}),
            ["object-restriction"],
        ),
        # Rule 5 again, for a file this reader cannot strip.
        (
            "a makefile holding a here-document is refused, not judged",
            EXIT_UNREADABLE,
            _fixture(base, "heredoc", {MAKEFILE: HEREDOC_MAKEFILE}),
            [],
        ),
        # Rule 4, the hygiene steps. Each of these leaves rule 1 satisfied, because
        # the file still pins a directory of its own for its other mode, which is
        # the whole reason this rule cannot be folded into rule 1.
        (
            "a run that never deletes the objects it did not build is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-object-delete", {RUNNER: NO_OBJECT_DELETE_RUNNER}),
            ["hygiene"],
        ),
        (
            "a run that never drops an unreadable profile buffer is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-buffer-drop", {RUNNER: NO_BUFFER_DROP_RUNNER}),
            ["hygiene"],
        ),
        (
            "a step named only in a trailing comment is a finding",
            EXIT_FINDINGS,
            _fixture(
                base, "commented-object-delete", {RUNNER: COMMENTED_OBJECT_DELETE_RUNNER}
            ),
            ["hygiene"],
        ),
        (
            "a real call carrying a trailing comment is not a finding",
            EXIT_OK,
            _fixture(base, "commented-and-called", {RUNNER: COMMENTED_AND_CALLED_RUNNER}),
            [],
        ),
        (
            "a run that writes its report before cleaning is a finding",
            EXIT_FINDINGS,
            _fixture(base, "hygiene-after", {RUNNER: HYGIENE_AFTER_REPORT_RUNNER}),
            ["hygiene", "hygiene"],
        ),
        # Rule 5, and the surface a rename would empty.
        (
            "a missing runner is a finding, not a pass",
            EXIT_FINDINGS,
            _fixture(base, "no-runner", {RUNNER: None}),
            ["surface"],
        ),
        (
            "a gate with no report step at all is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "no-report-builder",
                {RUNNER: _without_report_builder(CLEAN_RUNNER)},
            ),
            ["boundary"],
        ),
    ]


def _refusal_case(base: Path) -> Path | None:
    """A fixture whose only problem is a file this process cannot read.

    None when the refusal cannot be provoked here. A host that reads a mode-000
    file anyway reports the skip, because a case that cannot be set up is not a
    case that ran.
    """
    root = _fixture(base, "unreadable")
    blocked = root / MAKEFILE
    blocked.chmod(0o000)
    try:
        blocked.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return root
    return None


def _self_test() -> int:
    failures = 0
    total = 0
    skipped = 0
    with tempfile.TemporaryDirectory(prefix="coverage-surface-") as scratch:
        base = Path(scratch)
        for name, want, root, rules in _cases(base):
            total += 1
            found = scan(root)
            got = found.exit_code()
            fired = sorted(f.rule for f in found.findings)
            ok = got == want and fired == sorted(rules)
            print(
                f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want}; "
                f"rules {fired}, want {sorted(rules)})"
            )
            failures += 0 if ok else 1

        # The cases above read an exit code, and an exit code cannot say which
        # rule fired, or on which line, or what the check printed. The three
        # assertions below read the findings and the output, so a case cannot pass
        # because a different rule fired, because the fixture's own line was never
        # read, or because the finding said nothing.
        total += 1
        mixed = _fixture(base, "content", {RUNNER: NO_MODE_RUNNER})
        surface = scan(mixed)
        want_line = next(
            number
            for number, raw in enumerate((mixed / RUNNER).read_text().splitlines(), start=1)
            if raw.startswith(f"def {REPORT_BUILDER}")
        )
        got_where = [(f.path, f.line, f.rule) for f in surface.findings]
        ok = (
            surface.exit_code() == EXIT_FINDINGS
            and got_where == [(RUNNER, want_line, "failure-mode")]
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the failure-mode finding names the report "
            f"step's own line and nothing else (reported {got_where}, want "
            f"[('{RUNNER}', {want_line}, 'failure-mode')])"
        )
        failures += 0 if ok else 1

        total += 1
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            got = report(_fixture(base, "printed", {MAKEFILE: UNPROTECTED_MAKEFILE}))
        output = printed.getvalue()
        ok = (
            got == EXIT_FINDINGS
            and "coverage-surface-check: FINDING Makefile:2: object-restriction:" in output
            and "a package selector does " in output
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the whole check reports the unprotected "
            f"per-package command with its file, line, rule and reason (exit {got}, want "
            f"{EXIT_FINDINGS})"
        )
        failures += 0 if ok else 1

        total += 1
        whole = scan(_fixture(base, "whole", {MAKEFILE: WORKSPACE_ONE_COMMAND}))
        ok = (
            whole.exit_code() == EXIT_OK
            and not [f for f in whole.findings if f.rule in ("object-restriction", "boundary")]
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: a --workspace run doing tests and report "
            f"in one command is exempt from rules 1 and 2, with the failure mode the "
            f"only thing it has to satisfy (exit {whole.exit_code()}, want {EXIT_OK}, "
            f"findings {[str(f) for f in whole.findings]})"
        )
        failures += 0 if ok else 1

        root = _refusal_case(base)
        if root is None:
            total += 1
            skipped += 1
            print("self-test skipped: this process reads a mode-000 file, so the refusal cannot be provoked here")
        else:
            total += 1
            got = scan(root).exit_code()
            ok = got == EXIT_UNREADABLE
            print(
                f"self-test {'ok' if ok else 'FAIL'}: a Makefile this process cannot read fails "
                f"closed (exit {got}, want {EXIT_UNREADABLE})"
            )
            failures += 0 if ok else 1
            (root / MAKEFILE).chmod(0o600)

    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total - skipped} cases, {skipped} skipped)")
    return EXIT_OK


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return EXIT_OK
    root = Path(__file__).resolve().parent.parent
    if "--list-scanned" in argv:
        for relative in scan(root).scanned:
            print(relative)
        return EXIT_OK
    if argv:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    return report(root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))