"""Reading Makefile, workflow and script text with comments removed, and the variables a command line uses."""

from __future__ import annotations

import re
from pathlib import Path

#: Where the coverage commands are assembled for the make targets.
MAKEFILE = "Makefile"

#: The cargo executables reached by name. A protected `$(CARGO)` arrives carrying
#: its own name, so the comparison folds case: a make variable is upper case and
#: cargo is lower.
CARGO_EXECUTABLES = frozenset({"cargo", "cargo-llvm-cov"})

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
KIND_BY_SUFFIX = {".sh": KIND_SHELL, ".yml": KIND_YAML, ".yaml": KIND_YAML, ".mk": KIND_MAKE}

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
