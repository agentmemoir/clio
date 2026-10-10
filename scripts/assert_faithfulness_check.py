#!/usr/bin/env python3
"""Checks that a mechanical pass over assertions changed only what it was asked to.

A toolchain move made one lint an error, and the fix for it was mechanical: every
emptiness assertion gained a message naming the value it asserted on. Three
hundred and forty-two assertions across a hundred and seventy-three files were
edited by a script rather than by reading, and three defects came in. Every one of
them looked like finished work and none was visible in the diff.

An assertion was invented. A worker added an assertion at a line no lint had ever
flagged, and it failed on every run, because the field it asserted on is a budget
of one call by design and a spent budget is correct behaviour. An expression was
evaluated twice: where the condition was not first bound to a `let`, the message
argument called it again, so a clock read printed a number that was never asserted
on and a row-writing helper ran twice. Sixty-two identifiers were corrupted by a
rewrite that stripped three characters where it meant two.

All three were found by running something afterwards rather than by reading the
change. This check is that something, made permanent.

Rules, each comparing the working tree with a base commit:

1. No assertion was removed and no condition changed. Assertions are matched by
   the function that contains them, and within a function by position. The count
   of each kind must not fall, and every assertion that exists in both must have
   the same condition once whitespace is normalised. Adding a message is
   allowed, because a message is not what is asserted.

   The one shape this rule reads as equal is binding the value to a `let` first.
   `assert!(x.events().is_empty())` becoming `let events = x.events();` followed
   by `assert!(events.is_empty())` asserts the same thing, and it is the change
   rule 2 requires whenever the expression calls something: the value has to be
   bound before the message can name it without calling it a second time. Both
   sides are therefore compared with their `let` bindings put back, and a
   condition that still differs after that is a real change. Comparing the two
   texts as written would report every one of those as a broken assertion, which
   is a finding no reader can act on and a rule nobody could obey.
2. A message argument does not call anything. The rule the mechanical pass had to
   follow is that an expression containing a call is bound to a `let` first, and
   this is the rule that catches the place where it was not: a message argument
   that calls evaluates the call a second time. A bare identifier or a field path
   is fine. The condition is never judged by this rule; it did contain calls
   before the change and it does now.
3. An inline format capture names a plain identifier. Rust's implicit capture
   accepts an identifier and nothing else, so `"{report.rejected:?}"` does not
   compile. A positional message argument that is a bare identifier is the
   opposite mistake: it compiles, and the lint that names format arguments in
   place rejects it.
4. No assertion appeared where the base had none of that kind in that function.
   This is the rule that catches an invented assertion. A new `#[test]` function
   is fine, and so is a whole new file: neither is a claim about an assertion
   that used to exist.

Choosing the base:

- The whole tree. `--base` defaults to `origin/master`, and the changed files are
  the ones that differ from the merge base with it, plus the uncommitted ones, so
  an unstaged edit is judged rather than ignored.
- Staged content. `--staged` reads the index rather than the working tree, so a
  commit hook judges what is being committed and not what happens to be on disk
  afterwards.

A base that does not resolve is an error rather than an empty scope. A run that
compared against nothing would report clean having checked nothing, and the
scoped gate in this repository already fails closed on the same case.

Recording a finding as intended:

A finding is not always a mistake. An assertion whose value changed because the
thing it names changed is correct, and no rule here can tell that from a rewrite
that broke it. So a person who has read one records that judgement in
`scripts/assert_faithfulness_waivers.json`, and the finding is reported as waived
instead of failing the run. A path holds a list of entries, because one file can
raise two findings under one rule and a person who has read both records both.
Each entry names the condition the assertion now asserts, the reason it is
intended, and the line the finding reported. The line is only optional when the
file raised one finding with that claim: two findings carrying the same claim
under the same rule are told apart by their lines and by nothing else, so an entry
naming no line there cannot say which of them its author read.

Nothing else is weakened. An entry may only silence one finding, and only one its
author read, so four things about it are judged and each of them fails the run:

- an entry whose recorded text is neither the claim any finding this run reported
  nor one of the compared file's own assertions on the recorded line;
- an entry that recorded no line where more than one finding carries that claim,
  because an entry with no line cannot say which one was read;
- an entry whose recorded line is not the line that claim is reported on, which
  means it has slid onto a different assertion;
- an entry that matched a finding an earlier entry had already taken, which means
  one file raised more findings under this rule than the list records.

An entry can be spent: findings exist only while a condition differs from the
base, so an entry stops matching the moment its change merges, and a recorded
claim that is still one of the compared file's own assertions on the line the
entry records is reported as spent rather than stale. A landed entry can be
deleted; spent entries no longer fail the run, but the list grows until someone
deletes them. A claim the file now holds only on a different line stays a
failure, and its message names both lines so the record can be repaired or
deleted.

An entry for a file this run never compared is reported and judged by nothing. A
clean tree compares no file at all, so a run that had opened nothing has no
standing to call a record out of date. Those records are named on one line between
them rather than one line each, because the list grows one entry per judgement and a
line per entry would be printed on every unrelated run, which trains a reader to skip
the output that also carries the one line that matters. The line names a fixed number
of them and counts the rest, so it cannot grow with the list either.

The list itself is judged too. A list that is not there, or that cannot be read,
or that does not hold the shape described above, fails the run rather than reading
as a list holding nothing, because an absent list and an empty one are different
states and only one of them means nobody recorded anything. A list that is not a
regular file is refused before it is read, because the disk and the index cannot both
be right about one path: a checkout holds the link and the index holds the target's
name as text, so a run on the disk and a run judging what is being committed would
answer differently about one tree. A key written twice is refused like any other
wrong shape, because the parser keeps the last of the two and the first would go
missing with nothing to say that it had ever been written. A refusal of that kind
exits with the same code as an unreadable source file and says so on its own line,
because what is missing is not a path but the record every judgement was made in;
the findings are still reported, so the run shows it read the tree as well.

What it does not judge:

- A message a change left exactly as it found it. Rules 2 and 3 judge what a
  change introduced, so a pre-existing message is not this check's business and
  the clippy run already owns it.
- A function that was renamed, or moved to another file. Its assertions are no
  longer matched to anything, so they are skipped rather than guessed at. A test
  function the base held and the tree no longer does is named on its own line
  for a person to sign off on, and that line is a report rather than a verdict:
  no rule here can follow a rename, so failing the run over one would punish an
  honest rewrite. The commit that moved them is the one that knows.
- Rule 2 and rule 3 on an assertion that is itself new. A new assertion in a new
  function is not a change to an existing claim. The compiler judges whether it
  compiles, which is the half this check cannot do.

Usage:
  python3 scripts/assert_faithfulness_check.py
  python3 scripts/assert_faithfulness_check.py --base origin/master
  python3 scripts/assert_faithfulness_check.py --staged
  python3 scripts/assert_faithfulness_check.py --self-test
  python3 scripts/assert_faithfulness_check.py --list-scanned

Exit codes:
  0  every assertion that existed at the base still asserts the same thing, and
     every message a change added is well formed, apart from the findings whose
     change a person recorded as intended, which that line counts. A test
     function the base held and the tree no longer does is named on its own
     line for a person to sign off on, and that report never changes this code
  1  at least one finding, each naming the file and the line, or a recorded entry
     that matched no finding this run reported and whose claim the file no longer
     holds or holds only on another line. An entry whose claim is still one of the
     file's own assertions on the recorded line is spent: it is reported on its
     own line, and it never fails the run
  2  usage error
  3  this run is not a verdict, because something it had to read could not be read:
     a base that does not resolve, so nothing was compared at all; a source file it
     had to read, so the rest of the tree is still judged and those files are not;
     or the recorded list, which is not there, cannot be read, is not a regular
     file, or is a shape this reader refuses, so no judgement anyone recorded was
     honoured. The summary under it says which of those it was, and a run that
     refused the list still prints the findings it read
"""

from __future__ import annotations

import bisect
import contextlib
import io
import json
import re
import stat
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

USAGE = (
    "usage: assert_faithfulness_check.py [--base REF] [--staged] | --self-test | --list-scanned"
)

NAME = "assert-faithfulness"

#: The base a whole-tree run compares against. The remote-tracking form is the
#: default for the reason the scoped gate gives: a runner checks a pull request
#: out in detached HEAD, so a bare `master` does not resolve there.
DEFAULT_BASE = "origin/master"

#: The macros that take two values and compare them, so their condition is the
#: pair rather than the first argument.
COMPARING = frozenset({"assert_eq", "assert_ne", "debug_assert_eq", "debug_assert_ne"})

#: Every assertion macro whose condition is a claim, longest name first. `assert`
#: is a prefix of `assert_eq`, so a scan that tried the short name first would read
#: the longer one as a call and never see it.
MACRO_START = re.compile(
    r"(?<![A-Za-z0-9_:])"
    r"(assert_eq|assert_ne|debug_assert_eq|debug_assert_ne|debug_assert|assert)"
    r"[ \t]*!"
)

FN_START = re.compile(
    r"(?<![A-Za-z0-9_])"
    r"(?:(?:const|async|unsafe|extern)(?:\s+\"[^\"]*\")?\s+)*"
    r"fn\s+([A-Za-z_][A-Za-z0-9_]*)"
)

MOD_START = re.compile(r"(?<![A-Za-z0-9_])mod\s+([A-Za-z_][A-Za-z0-9_]*)")

IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

#: A raw string prefix: `r"..."`, `r#"..."#`, `br##"..."##`. The count of hashes
#: is the terminator, so the mask has to read it rather than guess at the first
#: quote after it.
RAW_STRING = re.compile(r"(?:b?r)(#*)\"")

#: The escape sequences inside a format string that change what the placeholder
#: scan sees. `\{` and `\}` would otherwise read as a placeholder that opens and
#: one that closes, and `\\` would end the string a character early.
MEANINGFUL_ESCAPES = {"\\": "\\", "{": "{", "}": "}", '"': '"'}

#: Where the recorded judgements live, as a path under the repository root. The
#: root handed to `scan` decides it rather than this file's own location: the
#: self-test judges throwaway repositories, and a path taken from `__file__`
#: would read this checkout's real list in the middle of a case.
WAIVERS_PATH = "scripts/assert_faithfulness_waivers.json"

#: The key the entries live under. Every other key beginning with an underscore is
#: documentation, which is the sibling list's convention: the reason the list
#: exists is longer than a JSON string should carry, and the person who has to
#: read it is a reader of the file rather than of a run.
WAIVERS_KEY = "_waivers"

#: Every rule id this check emits, in one place. The reader refuses a list naming
#: anything else and the self-test's own pin of the six names reads this, because two
#: lists of the same six strings can drift and a drifted pair is silent: a rule added
#: here without being emitted makes every record naming it inert, and a rule emitted
#: without being added here makes every record naming it a run that fails for a reason
#: no run printed.
RULE_IDS = frozenset(
    {
        "R1-changed",
        "R1-removed",
        "R2-call",
        "R3-capture",
        "R3-positional",
        "R4-added",
    }
)

#: How many records the line about the ones a run did not judge names. That line
#: exists so a reader learns that some records went unjudged and can see the shape of
#: the list; five rule-and-path pairs are enough to show both, and past five the next
#: move is to open the committed list, which is the record itself. What the line left
#: out is counted beside it, so the cap costs nothing but the names.
NOT_EXERCISED_SHOWN = 5

#: How many of the paths a run did compare the same line names beside the records it
#: did not judge. Two is enough to show the recorded path and a real one side by side,
#: which is the whole point of naming them, and the number is fixed rather than read
#: from the run so that the line cannot grow with the size of a change.
COMPARED_SHOWN = 2


class Finding:
    """One thing that is wrong, where it was found, and what it is about."""

    def __init__(self, path: str, line: int, rule: str, message: str, claim: str) -> None:
        self.path = path
        self.line = line
        self.rule = rule
        self.message = message
        #: What this finding is about, in the words the assertion itself uses, so
        #: that a judgement recorded against it can be found again the next time
        #: the same assertion fails. The scan has already normalised the
        #: condition, so nothing here normalises it a second time. Required
        #: rather than defaulted, because a site that left it out would otherwise
        #: report a finding carrying a claim that nothing could ever match.
        self.claim = claim

    def __str__(self) -> str:
        where = f"{self.path}:{self.line}" if self.line else self.path
        return f"{where}: {self.rule}: {self.message}"


class Assertion:
    """One assertion invocation: what it asserts, and what it says when it fails."""

    def __init__(
        self,
        macro: str,
        line: int,
        condition: str,
        message: list[str],
        expanded: str = "",
    ) -> None:
        self.macro = macro
        self.line = line
        self.condition = condition
        self.message = message
        #: The condition with every `let` it names put back, so two assertions
        #: that differ only in whether the value was bound compare equal.
        self.expanded = expanded or condition


class Surface:
    """The files this check had to read, and the verdict on each."""

    def __init__(self) -> None:
        self.findings: list[Finding] = []
        self.unreadable: list[str] = []
        self.scanned: list[str] = []
        #: Findings a person has recorded as intended. They are not failures, so
        #: they cannot sit in `findings`: the exit code reads that list as
        #: something wrong, and the count a reader sees would be counting a
        #: decision as a defect.
        self.waived: list[Finding] = []
        #: Recorded entries that matched no finding this run, which means the
        #: record no longer describes the tree. This is a failure, and it is in
        #: neither list above on purpose: it is not a finding about the code, and
        #: `unreadable` carries a message saying the run does not cover the paths
        #: it names, which would be false here.
        self.stale_waivers: list[str] = []
        #: Recorded entries whose claim is still one of the compared file's own
        #: assertions on the line the entry records, so the change they waived
        #: has landed. Not a failure: the entry no longer judges anything, and it
        #: stays out of `exit_code` until someone deletes it from the list.
        self.spent_waivers: list[str] = []
        #: Recorded entries for a file this run never compared, so nothing here
        #: can say whether they are still needed. Reported, never a failure: a
        #: run that never looked at a file cannot judge what was recorded for it.
        #: Each item is the rule and the path rather than a whole sentence, because
        #: one line is printed for the lot and names these.
        self.unexercised_waivers: list[str] = []
        #: Test functions the base held and the working tree no longer does, so
        #: they were removed, renamed, or moved to another file. This is a report
        #: for a person to sign off on, not a finding: no rule here can follow a
        #: rename, so failing the run over one would punish an honest rewrite,
        #: and staying silent is the blind spot this line exists to name.
        self.removed_tests: list[str] = []

    def exit_code(self) -> int:
        """The verdict. A base or file this check could not read outranks all else.

        A clean run over the files it did reach is not a clean run, and reporting
        it as one is the failure this check exists to prevent.
        """
        if self.unreadable:
            return EXIT_UNREADABLE
        # A recorded judgement that matched nothing is the next thing that fails
        # the run. It reports under the findings code rather than one of its own,
        # because a caller that only knows the existing codes still has to be
        # told. An entry for a file this run never compared is not judged, so it
        # stays out of the verdict.
        if self.stale_waivers:
            return EXIT_FINDINGS
        return EXIT_FINDINGS if self.findings else EXIT_OK


# --- Reading the source ---------------------------------------------------------


def mask(text: str) -> str:
    """A copy of `text` with every comment and literal body blanked out.

    Offsets are preserved and newlines survive, so anything that has to know
    where it is can scan the mask while anything that has to know what it says
    reads the original. Blank space is used rather than a placeholder because a
    placeholder would be a character that a brace or comma scan could mistake for
    code -- a `,` inside a string literal would split a macro's arguments in the
    wrong place, and a `(` inside one would read as the call rule looks for.
    """
    out = list(text)
    index = 0
    length = len(text)
    while index < length:
        char = text[index]
        if char == "/" and text.startswith("//", index):
            while index < length and text[index] != "\n":
                out[index] = " "
                index += 1
            continue
        if char == "/" and text.startswith("/*", index):
            depth = 0
            while index < length:
                if text.startswith("/*", index):
                    depth += 1
                    out[index] = out[index + 1] = " "
                    index += 2
                    continue
                if text.startswith("*/", index):
                    depth -= 1
                    out[index] = out[index + 1] = " "
                    index += 2
                    if depth == 0:
                        break
                    continue
                if text[index] != "\n":
                    out[index] = " "
                index += 1
            continue
        raw = RAW_STRING.match(text, index)
        if raw is not None:
            index = _blank(text, out, index, index + len(raw.group(0)))
            index = _blank_string(text, out, index, '"' + raw.group(1), closing=False)
            continue
        if char == '"':
            index = _blank_string(text, out, index, '"', closing=True)
            continue
        if char == "'":
            end = _char_literal_end(text, index)
            if end is not None:
                for position in range(index, end):
                    if text[position] != "\n":
                        out[position] = " "
                index = end
                continue
        index += 1
    return "".join(out)


def _blank(text: str, out: list[str], start: int, end: int) -> int:
    """Blank `[start, end)` and return `end`."""
    for position in range(start, min(end, len(text))):
        if text[position] != "\n":
            out[position] = " "
    return end


def _blank_string(text: str, out: list[str], index: int, terminator: str, closing: bool) -> int:
    """Blank a string body starting after its opening quote, returning the end."""
    length = len(text)
    if closing:
        quote = index
        index += 1
        while index < length:
            char = text[index]
            if char == "\\":
                _blank(text, out, index, min(index + 2, length))
                index += 2
                continue
            if char == '"':
                _blank(text, out, quote, index + 1)
                return index + 1
            if char == "\n":
                break
            index += 1
        _blank(text, out, quote, index)
        return index
    # `index` already sits past the opening quote, so the body starts here.
    found = text.find(terminator, index)
    if found == -1:
        _blank(text, out, index, length)
        return length
    _blank(text, out, index, found + len(terminator))
    return found + len(terminator)


def _char_literal_end(text: str, index: int) -> int | None:
    """Where the character literal at `index` ends, or None for a lifetime.

    A lifetime is a quote followed by a name and something that is not a closing
    quote. Reading it as a literal would blank the rest of the line, and a
    function signature carrying one is ordinary Rust.
    """
    length = len(text)
    if index + 1 >= length:
        return None
    if text[index + 1] == "\\":
        closing = text.find("'", index + 2)
        return closing + 1 if closing != -1 and "\n" not in text[index : closing] else None
    if text[index + 2 : index + 3] == "'":
        return index + 3
    return None


def normalise(text: str) -> str:
    """One canonical spelling of an expression, for comparing two of them.

    Three differences are erased, and each one changed nothing that was asserted
    while reading as a different assertion:

    - Whitespace next to punctuation is dropped, and collapsed to one space
      elsewhere. rustfmt starts a method chain's dots on a new line, so the same
      expression reads `a.b()` on one line and `a .b()` when it is wrapped.
    - A trailing comma before a closing bracket goes. rustfmt writes one when an
      argument list is laid out over lines and omits it when it is not.
    - Nothing inside a string literal or a comment is touched. Treating a string
      as code once turned `"clean lesson: cache before query"` into
      `"clean lesson:cache before query"`, and it did so silently, because the
      whitespace rule cannot tell a colon in code from a colon in a message.

    The literal rule is why this walks the mask rather than joining words: the
    spacing inside `"a, b"` is part of the value and the comma inside it is not a
    separator. The text is cut into pieces first and joined afterwards, because
    whether a space survives depends on both of its neighbours and a single pass
    cannot see the one behind it. Nothing is evaluated here, so two spellings can
    only collide if they were already the same expression.
    """
    pieces = _pieces(text)
    out: list[str] = []
    for index, piece in enumerate(pieces):
        if index:
            before = out[-1][-1] if out[-1] else ""
            after = piece[0]
            out.append(" " if _is_word(before) and _is_word(after) else "")
        out.append(piece)
    return "".join(out)


def _pieces(text: str) -> list[str]:
    """Cut an expression into the pieces that must stay apart when joined.

    A string literal, a character literal, a raw string or a comment is one piece,
    copied exactly, because its spacing is part of its value. Everything else is
    split on whitespace, and a comma that only separates the last argument from
    its closing bracket is dropped.

    The literals are found by reading the text, not by asking the mask. The mask
    blanks a literal's body, so a blanked run could be a literal or a comment or
    a space that was already there, and reading it as a literal turned every
    single character of ordinary code into one.
    """
    found: list[str] = []
    index = 0
    length = len(text)
    while index < length:
        if text.startswith("//", index):
            end = text.find("\n", index)
            end = length if end == -1 else end
            found.append(text[index:end])
            index = end
            continue
        if text.startswith("/*", index):
            end = text.find("*/", index + 2)
            end = length if end == -1 else end + 2
            found.append(text[index:end])
            index = end
            continue
        raw = RAW_STRING.match(text, index)
        if raw is not None:
            terminator = '"' + raw.group(1)
            end = text.find(terminator, raw.end())
            end = length if end == -1 else end + len(terminator)
            found.append(text[index:end])
            index = end
            continue
        if text[index] == '"':
            end = index + 1
            while end < length:
                if text[end] == "\\":
                    end += 2
                    continue
                if text[end] == '"' or text[end] == "\n":
                    end += 1
                    break
                end += 1
            found.append(text[index:end])
            index = end
            continue
        if text[index] == "'":
            end = _char_literal_end(text, index)
            if end is not None:
                found.append(text[index:end])
                index = end
                continue
        char = text[index]
        if char.isspace():
            index += 1
            continue
        if char == ",":
            ahead = index + 1
            while ahead < length and text[ahead].isspace():
                ahead += 1
            if ahead < length and text[ahead] in ")]}":
                index += 1
                continue
        # A run of word characters is one piece, not one piece per letter. The
        # join puts a space between two words, and `store` cut into five pieces
        # came back as `s t o r e`.
        if _is_word(char):
            end = index
            while end < length and _is_word(text[end]):
                end += 1
            found.append(text[index:end])
            index = end
            continue
        found.append(char)
        index += 1
    return found


def _is_word(char: str) -> bool:
    """Whether a character is part of a name or a number."""
    return bool(char) and (char.isalnum() or char == "_")


def _match(masked: str, index: int, opener: str, closer: str) -> int:
    """The index after the bracket matching the one at `index`, or -1."""
    depth = 0
    length = len(masked)
    while index < length:
        char = masked[index]
        if char == opener:
            depth += 1
        elif char == closer:
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    return -1


def _split_arguments(text: str) -> list[str]:
    """Split a macro's argument text on the commas at its outermost level.

    This is what the compiler does with a macro's argument list, so a comma inside
    a nested call is not a separator and a comma inside a literal is not one
    either. The mask decides which commas count.
    """
    masked = mask(text)
    parts: list[str] = []
    depth = 0
    start = 0
    for index, char in enumerate(masked):
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        elif char == "," and depth == 0:
            parts.append(text[start:index])
            start = index + 1
    parts.append(text[start:])
    return parts


def scopes(masked: str) -> list[tuple[str, int, int, int]]:
    """Every function body in a masked file, as (key, body start, body end, fn start).

    The key is the module path and the function name, because two modules may
    each hold a helper of the same name and matching them by name alone would
    compare one module's assertions against another's. The path is taken from the
    enclosing `mod` blocks rather than from the file's own name, because a test
    module is declared from the file it belongs to.

    The fourth field is where the `fn` keyword sits, so a caller can read the
    attributes above the signature and tell a test function from an ordinary one.
    """
    found: list[tuple[str, int, int, int]] = []
    stack: list[dict[str, object]] = []
    pending: tuple[str, str, int] | None = None
    angle = 0
    index = 0
    length = len(masked)
    while index < length:
        char = masked[index]
        if char == "f":
            match = FN_START.match(masked, index)
            if match is not None:
                pending = ("fn", match.group(1), index)
                angle = 0
        elif char == "m":
            match = MOD_START.match(masked, index)
            if match is not None:
                pending = ("mod", match.group(1), 0)
                angle = 0
        elif char == "<":
            if pending is not None:
                angle += 1
        elif char == ">":
            if pending is not None and angle > 0:
                angle -= 1
        elif char == ";":
            pending = None
            angle = 0
        elif char == "{":
            # A `{N}` inside a generic bound is not a body. Without this, a
            # signature carrying one would close its own body early and the
            # assertions after it would belong to the module.
            if not (pending is not None and pending[0] == "fn" and angle > 0):
                stack.append(
                    {
                        "kind": pending[0] if pending else None,
                        "name": pending[1] if pending else "",
                        "fn_start": pending[2] if pending else 0,
                        "path": tuple(f["name"] for f in stack if f["kind"] == "mod"),
                        "start": index,
                    }
                )
                pending = None
                angle = 0
        elif char == "}":
            if stack:
                frame = stack.pop()
                if frame["kind"] == "fn":
                    path = ".".join(frame["path"])  # type: ignore[arg-type]
                    key = f"{path}::{frame['name']}" if path else str(frame["name"])
                    found.append((key, frame["start"], index + 1, frame["fn_start"]))
        index += 1
    return found


def _is_test(masked: str, fn_start: int) -> bool:
    """Whether the function whose `fn` keyword sits at `fn_start` carries `#[test]`.

    The attribute is read off the lines above the signature. An attribute line is
    walked past, because `#[test]` may sit above `#[should_panic]`, and so is a
    comment or a blank line. Anything else is code, so the walk stops: a `#[test]`
    further up belongs to another function.
    """
    index = masked.rfind("\n", 0, fn_start) + 1
    line = masked[index:].split("\n", 1)[0].strip()
    if "#[test]" in line:
        return True
    while index > 0:
        previous = masked.rfind("\n", 0, index - 1) + 1
        line = masked[previous:index].strip()
        if not line or line.startswith("//") or line.startswith("#"):
            if "#[test]" in line:
                return True
            index = previous
            continue
        return False
    return False


def test_functions(text: str) -> set[str]:
    """Every test function in a file, named the way `scopes` names them.

    A function is a test when it carries `#[test]` or when a module named `tests`
    encloses it. Both are the shapes test code takes, and a helper inside a test
    module is part of the same surface, so its removal is worth a reader's
    attention too.
    """
    masked = mask(text)
    found: set[str] = set()
    for key, _, _, fn_start in scopes(masked):
        if "tests" in key.replace("::", ".").split(".") or _is_test(masked, fn_start):
            found.add(key)
    return found


#: `let name = expression;` and `let mut name = expression;`. The name is required
#: and the `=` is taken as the first one after it, because a type ascription may
#: hold a `>` and a closure may hold an `=`.
LET_BINDING = re.compile(
    r"(?<![A-Za-z0-9_])let\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\b[^=;]*="
)

#: A bare identifier not preceded by a field access or a path separator and not
#: followed by an argument list. This is where substituting a binding back into an
#: expression is safe, and `sink.events` is a field rather than the local.
LOCAL_USE = re.compile(
    r"(?<![A-Za-z0-9_:.])"
    r"([A-Za-z_][A-Za-z0-9_]*)"
    r"(?![A-Za-z0-9_(])"
)


def bindings_in(text: str) -> dict[int, list[tuple[int, str, str]]]:
    """Every `let name = expression;` in a file, as {function start: [(offset, name, expr)]}.

    The body is taken by matching brackets from the `=`, so an expression holding
    its own call, closure or collection still binds whole.

    Two properties this has to have, and both were learned from a real tree. The
    bindings are per function, because two functions may each bind `count` to
    something different and a file-wide map substitutes one into the other. And
    they are kept in written order with their offsets, because a function may bind
    one name twice -- `let noop = detach(a); ... let noop = detach(b);` -- and the
    assertion after the second one means the second expression while the assertion
    between them means the first. A map that kept only the last binding made both
    assertions read as the second one, so the first looked like a changed claim.
    """
    masked = mask(text)
    found: dict[int, list[tuple[int, str, str]]] = {}
    function_starts = sorted(start for _, start, _, _ in scopes(masked))
    for match in LET_BINDING.finditer(masked):
        index = match.end()
        while index < len(masked) and masked[index] in " \t\n\r":
            index += 1
        end = _match_expression(masked, index)
        if end is None:
            continue
        expression = normalise(text[index:end])
        if not expression:
            continue
        position = bisect.bisect_right(function_starts, match.start()) - 1
        found.setdefault(function_starts[position] if position >= 0 else -1, []).append(
            (match.start(), match.group(1), expression)
        )
    return found


def in_scope(bindings: list[tuple[int, str, str]], offset: int) -> dict[str, str]:
    """The bindings visible at `offset`, later ones winning over earlier ones."""
    visible: dict[str, str] = {}
    for start, name, expression in bindings:
        if start < offset:
            visible[name] = expression
    return visible


def _match_expression(masked: str, index: int) -> int | None:
    """Where the expression starting at `index` ends, at a `;` or a `}`."""
    depth = 0
    length = len(masked)
    while index < length:
        char = masked[index]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            if depth == 0:
                return None
            depth -= 1
        elif char == ";" and depth == 0:
            return index
        index += 1
    return None


#: Which bracket closes which.
CLOSING = {"(": ")", "[": "]", "{": "}"}


def hoist(condition: str, bindings: dict[str, str], rounds: int = 8) -> str:
    """Put back every binding the condition names, so two shapes can be compared.

    Binding an expression to a `let` and asserting on the name is a faithful
    change to the assertion, and it is the change this repository's own rule asks
    for wherever the expression calls something: a message argument must not call
    it twice, so the value has to be bound before either use. Comparing the text
    of the two conditions would report every one of those as a changed claim,
    which is a finding a reader cannot act on and a rule that cannot be obeyed.

    Only a whole-identifier use is replaced, never one that is a field name or a
    path segment, so a field that happens to share a local's name is left alone.
    Substitution does descend into brackets: binding a whole call and then binding
    part of what it returned is the natural way to do this in two steps, and
    stopping at the bracket left the inner name unresolved and reported a faithful
    rewrite as a changed claim.

    The passes repeat until the text stops moving, because a binding may name
    another binding. The bound is a safety net against a binding that names itself.
    """
    if not bindings:
        return normalise(condition)
    current = normalise(condition)
    for _ in range(rounds):
        replaced = _expand_once(current, bindings)
        if replaced == current:
            break
        current = replaced
    # One more stage, this time walking into the brackets, and repeated until it
    # settles. The passes above stop at a bracket, which is what keeps a binding's
    # own arguments intact, so a name that arrived inside one is still unresolved:
    # `let hid = fake.hide();` then `let taken = identity(&hid);` leaves `hid`
    # inside parentheses, and a chain of any length needs one step per link.
    # Resolving it here rather than in the loop means this stage reads only what a
    # substitution introduced, never what the assertion itself wrote into a call.
    for _ in range(rounds):
        deeper = _resolve_inside(current, bindings)
        if deeper == current:
            break
        current = deeper
    return current


def _resolve_inside(condition: str, inner: dict[str, str]) -> str:
    """Substitute names that sit inside a bracket, leaving other text alone."""
    masked = mask(condition)
    pieces: list[str] = []
    index = 0
    length = len(condition)
    while index < length:
        char = masked[index]
        if char in "([{":
            end = _match(masked, index, char, CLOSING[char])
            if end == -1:
                pieces.append(condition[index:])
                break
            body = condition[index + 1 : end - 1]
            if _has_binding(body, inner):
                pieces.append(char + _expand_once(body, inner) + CLOSING[char])
            else:
                pieces.append(condition[index:end])
            index = end
            continue
        pieces.append(char)
        index += 1
    return "".join(pieces)


def _has_binding(text: str, bindings: dict[str, str]) -> bool:
    """Whether any name in `text` is one `bindings` can resolve."""
    masked = mask(text)
    for match in LOCAL_USE.finditer(masked):
        if match.group(1) in bindings:
            return True
    return False


def _expand_once(condition: str, bindings: dict[str, str]) -> str:
    """One pass of `hoist`: substitute every binding the expression names.

    Brackets are walked, so a name an assertion passed as an argument still
    resolves: `let positions = indices(&order)` needs `order` expanded inside its
    own parentheses. The text a substitution brings is not walked in the same pass,
    because those arguments belong to the line that bound the value rather than to
    the assertion, and the passes repeat until the text settles.
    """
    masked = mask(condition)
    pieces: list[str] = []
    index = 0
    length = len(condition)
    while index < length:
        char = masked[index]
        if char in "([{":
            end = _match(masked, index, char, CLOSING[char])
            if end == -1:
                pieces.append(condition[index:])
                break
            pieces.append(condition[index:end])
            index = end
            continue
        if (char.isalpha() or char == "_") and (index == 0 or masked[index - 1] not in ".:"):
            name_end = index
            while name_end < length and (
                masked[name_end].isalnum() or masked[name_end] == "_"
            ):
                name_end += 1
            name = condition[index:name_end]
            following = masked[name_end] if name_end < length else ""
            # An explicit membership test rather than `not in "(!"`: an empty
            # string is a substring of every string in Python, so the condition at
            # the end of an expression -- `called`, the commonest case there is --
            # would read as a call and never be substituted.
            # Substituted without parentheses. The parenthesised form compared
            # unequal to the same expression written inline -- `(a.b()).c()` is
            # not the text of `a.b().c()` -- so the two sides of an honest pair
            # never met. Nothing is evaluated here, so the parentheses were only
            # ever a way of losing the comparison.
            if name in bindings and following not in {"(", "!"}:
                pieces.append(bindings[name])
            else:
                pieces.append(name)
            index = name_end
            continue
        pieces.append(char)
        index += 1
    return normalise("".join(pieces))


def function_of(found: list[tuple[str, int, int, int]], offset: int) -> str:
    """The innermost function containing `offset`, or "" when none does."""
    best_key = ""
    best_start = -1
    for key, start, end, _ in found:
        if start < offset <= end and start > best_start:
            best_key = key
            best_start = start
    return best_key


def line_of(text: str, offset: int) -> int:
    """The one-based line `offset` falls on."""
    return text.count("\n", 0, offset) + 1


def assertions_in(text: str) -> dict[str, list[Assertion]]:
    """Every assertion in one file, grouped by the function that contains it.

    Every function appears, holding an empty list when it holds no assertion.
    That is what makes rule 4 possible at all: a function the base had and the
    working tree now has assertions in is exactly the invented-assertion case,
    and a grouping that only listed functions carrying assertions could not tell
    that function from one that did not exist at the base.
    """
    masked = mask(text)
    found = scopes(masked)
    bindings = bindings_in(text)
    starts = sorted(start for _, start, _, _ in found)
    grouped: dict[str, list[Assertion]] = {key: [] for key, _, _, _ in found}
    for match in MACRO_START.finditer(masked):
        index = match.end()
        while index < len(masked) and masked[index] in " \t\n\r":
            index += 1
        if index >= len(masked) or masked[index] != "(":
            continue
        end = _match(masked, index, "(", ")")
        if end == -1:
            continue
        parts = [part.strip() for part in _split_arguments(text[index + 1 : end - 1])]
        while parts and parts[-1] == "":
            parts.pop()
        if not parts:
            continue
        macro = match.group(1)
        if macro in COMPARING:
            condition = " == ".join(normalise(part) for part in parts[:2])
            message = parts[2:]
        else:
            condition = normalise(parts[0])
            message = parts[1:]
        key = function_of(found, match.start())
        here = bisect.bisect_right(starts, match.start()) - 1
        local = in_scope(bindings.get(starts[here] if here >= 0 else -1, []), match.start())
        grouped.setdefault(key, []).append(
            Assertion(
                macro,
                line_of(text, match.start()),
                condition,
                message,
                hoist(condition, local),
            )
        )
    return grouped


# --- Judging one message --------------------------------------------------------


def literal_body(text: str) -> str | None:
    """The contents of `text` when it is one string literal, else None.

    Escapes are resolved only where the change matters. A doubled backslash, a
    quote, and a brace decide what a placeholder scan sees; every other escape is
    left as written because a newline or a tab cannot open a placeholder.
    """
    body = text.strip()
    raw = RAW_STRING.match(body)
    if raw is not None:
        terminator = '"' + raw.group(1)
        start = len(raw.group(0))
        if not body.startswith(terminator, start):
            return None
        rest = body[start:]
        if not rest.endswith(terminator) or len(rest) < 2 * len(terminator):
            return None
        return rest[: -len(terminator)]
    if not (body.startswith('"') and body.endswith('"') and len(body) >= 2):
        return None
    inner = body[1:-1]
    out: list[str] = []
    index = 0
    while index < len(inner):
        char = inner[index]
        if char == "\\" and index + 1 < len(inner):
            following = inner[index + 1]
            out.append(MEANINGFUL_ESCAPES.get(following, "\\" + following))
            index += 2
            continue
        out.append(char)
        index += 1
    return "".join(out)


def placeholders(format_text: str) -> list[str]:
    """The body of every `{...}` in a format string, `{{` and `}}` excluded.

    The body is what precedes the first `:`, so a format spec is not read as part
    of the name. A body may be empty or a number, both of which are positional.
    """
    bodies: list[str] = []
    index = 0
    length = len(format_text)
    while index < length:
        char = format_text[index]
        if char in "{}":
            doubled = format_text.startswith(char * 2, index)
            if doubled:
                index += 2
                continue
            if char == "{":
                close = format_text.find("}", index)
                if close == -1:
                    break
                bodies.append(format_text[index + 1 : close].split(":", 1)[0].strip())
                index = close + 1
                continue
            index += 1
            continue
        index += 1
    return bodies


def called_name(text: str) -> str | None:
    """The name of the first call in `text`, or None when it calls nothing.

    A call is an identifier followed by an argument list, with `::<...>` and a
    macro's `!` allowed in between. A macro invocation counts as a call whether
    its arguments are bracketed or parenthesed, because `format!(..)` and
    `vec![..]` both evaluate their arguments and both would evaluate a second
    time here. A field path and an index have no parenthesis after them, so
    `report.keys` and `members[1].1` are not calls. The text is masked first, so
    a parenthesis inside a string is not one either.
    """
    masked = mask(text)
    index = 0
    length = len(masked)
    while index < length:
        if not (masked[index].isalpha() or masked[index] == "_"):
            index += 1
            continue
        name_end = index
        while name_end < length and (masked[name_end].isalnum() or masked[name_end] == "_"):
            name_end += 1
        probe = name_end
        bang = False
        while True:
            while probe < length and masked[probe] in " \t\n\r":
                probe += 1
            if probe < length and masked[probe] == "!":
                probe += 1
                bang = True
                continue
            if masked.startswith("::<", probe):
                angle_end = _match_angle(masked, probe + 2)
                if angle_end == -1:
                    return text[index:name_end]
                probe = angle_end
                continue
            break
        if probe < length and (
            masked[probe] == "(" or (bang and masked[probe] in "[{")
        ):
            return text[index:name_end]
        index = name_end
    return None


def _match_angle(masked: str, index: int) -> int:
    """The index after the `>` matching the `<` at `index`, or -1."""
    depth = 0
    length = len(masked)
    while index < length:
        char = masked[index]
        if char == "<":
            depth += 1
        elif char == ">":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    return -1


def message_problems(message: list[str]) -> list[tuple[str, str]]:
    """Every rule 2 and rule 3 problem in one message, as (rule, what)."""
    problems: list[tuple[str, str]] = []
    for argument in message:
        called = called_name(argument)
        if called is not None:
            problems.append(
                (
                    "R2-call",
                    f"`{called}` is called again inside the message, so the printed "
                    "value is not the value that was asserted on. Bind the "
                    "expression to a `let` and name that instead",
                )
            )
    if not message:
        return problems
    fmt = literal_body(message[0])
    if fmt is None:
        return problems
    bodies = placeholders(fmt)
    positional = 0
    for body in bodies:
        if body == "" or body.isdigit():
            positional += 1
            continue
        if IDENTIFIER.fullmatch(body) is None:
            problems.append(
                (
                    "R3-capture",
                    f"`{{{body}}}` is an inline capture naming more than an "
                    "identifier, and rustc's implicit capture accepts an "
                    f"identifier and nothing else, so it does not compile. Write "
                    f"`{{:?}}` and pass `{body}` as an argument",
                )
            )
    if positional:
        for argument in message[1:]:
            if IDENTIFIER.fullmatch(normalise(argument)) is not None:
                problems.append(
                    (
                        "R3-positional",
                        f"`{argument}` is a bare identifier passed where the format "
                        "string has a positional placeholder. The lint that names "
                        "format arguments in place rejects it, so write it into "
                        "the format string instead",
                    )
                )
    return problems


# --- Comparing with the base ----------------------------------------------------


def _git(root: Path, *args: str) -> tuple[int, str]:
    """Run a git command in `root` and return its status and output."""
    done = subprocess.run(
        ["git", *args],  # noqa: S603
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    return done.returncode, done.stdout


def resolve_base(root: Path, base: str) -> str | None:
    """The spelling of `base` this checkout can resolve, or None.

    A bare branch name does not resolve on a runner that checked the pull
    request out in detached HEAD, where only the remote-tracking ref exists, so
    the remote form is tried second. Both failing is a typo or a missing fetch,
    and neither may become an empty scope.
    """
    for candidate in (base, f"origin/{base}"):
        status, _ = _git(root, "rev-parse", "--verify", "--quiet", f"{candidate}^{{commit}}")
        if status == 0:
            return candidate
    return None


def changed_files(root: Path, base_sha: str, staged: bool) -> list[str]:
    """Every Rust file this run has to compare, in either mode."""
    if staged:
        paths = [
            line
            for line in _git(root, "diff", "--cached", "--name-only")[1].splitlines()
            if line.endswith(".rs")
        ]
        return sorted(dict.fromkeys(paths))
    paths: set[str] = set()
    for args in (
        ("diff", "--name-only", f"{base_sha}..HEAD"),
        ("diff", "--name-only", "--cached"),
        ("diff", "--name-only"),
    ):
        paths.update(line for line in _git(root, *args)[1].splitlines() if line.endswith(".rs"))
    for entry in _git(root, "ls-files", "--others", "--exclude-standard", "-z")[1].split("\0"):
        if entry.endswith(".rs"):
            paths.add(entry)
    return sorted(paths)


def read_base(root: Path, sha: str, relative: str, surface: Surface) -> str | None:
    """One file as the base commit had it, or None when there was none."""
    status, output = _git(root, "show", f"{sha}:{relative}")
    if status != 0:
        return None
    return output


def read_work(root: Path, relative: str, staged: bool, surface: Surface) -> str | None:
    """One file as it is now: from the index in staged mode, else from disk."""
    if staged:
        status, output = _git(root, "show", f":{relative}")
        if status != 0:
            return None
        return output
    try:
        return (root / relative).read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        surface.unreadable.append(f"{relative}: {error.strerror or error}")
        return None


# --- Recorded judgements -------------------------------------------------------


def _list_is_regular(root: Path) -> bool | None:
    """Whether the recorded list is a regular file, or None when there is none.

    A link is refused rather than followed. One path cannot be right in both places:
    a checkout holds the link, and the index holds the target's name as text, so a run
    on the disk honours what the link points at while the run reading the index refuses
    it. That disagreement is the whole reason the record has to be a file, because the
    run that judges what is being committed is the one nobody gets to see locally.

    The check is here rather than in the reader every source file goes through: a
    linked source file is ordinary, and only the recorded list has to survive being
    committed.
    """
    try:
        return stat.S_ISREG((root / WAIVERS_PATH).lstat().st_mode)
    except OSError:
        return None


def read_waivers(
    root: Path, staged: bool, surface: Surface
) -> dict[str, dict[str, list[tuple[str, int | None]]]]:
    """Every recorded entry as rule, then path, then a list of condition and line.

    A list this run cannot read is a failure, and a list that is not there is the
    case that matters, because an absent list and a list holding nothing are
    different states: the first means the record of every judgement a person made
    is gone, and reporting clean over a record that was deleted is the one thing
    this check must never do. A list is committed, so a checkout that has none did
    not record nothing, it lost something.

    A list that is there and cannot be read fails the same way, and for the same
    reason. Read in part it would waive findings nobody read; read not at all it
    would make every run fail for a reason no run printed. One that is not a regular
    file is refused before it is read at all, in both modes, so the two runs over one
    tree cannot answer differently.

    It comes from the index in staged mode for the reason the source files do: a
    commit that adds an entry without staging it has to fail here, where the
    person can still see it, rather than pass now and fail once it is committed.
    """
    before = len(surface.unreadable)
    if _list_is_regular(root) is False:
        surface.unreadable.append(
            f"{WAIVERS_PATH}: the recorded list is not a regular file, so it cannot "
            "be read the same way twice. A link is followed by a run on the disk and "
            "stored as its own text in the index, so one of the two runs would be "
            "answering about a path rather than about a list"
        )
        return {}
    text = read_work(root, WAIVERS_PATH, staged, surface)
    if text is None:
        # A file that is not in the index reads the same as a file that is there
        # and unreadable, and `read_work` says nothing about the first, so the
        # count decides whether this is the second case or the first.
        if len(surface.unreadable) == before:
            surface.unreadable.append(
                f"{WAIVERS_PATH}: there is no recorded list here, so no judgement "
                "recorded by anyone was honoured"
            )
        return {}
    try:
        return _entries(json.loads(text, object_pairs_hook=_object_pairs))
    except ValueError as error:
        surface.unreadable.append(f"{WAIVERS_PATH}: {error}")
        return {}


class _Repeated(dict):
    """A JSON object that remembers any key written twice in it.

    The parser keeps the last of two identical keys and says nothing at all, so a
    record written twice would go missing and the finding it was written for would go
    stale with nothing to say that a record had ever been there. The parser cannot be
    asked to refuse, so the object carries the news and the reader refuses once it is
    already holding the rule and the path, which is what puts the repeat next to the
    layer it sits under rather than nowhere at all.
    """

    def __init__(self, pairs: list[tuple[str, object]]) -> None:
        super().__init__()
        #: Each repeated key once, in the order it was first written.
        self.repeated: list[str] = []
        for key, value in pairs:
            if key in self and key not in self.repeated:
                self.repeated.append(key)
            self[key] = value


def _object_pairs(pairs: list[tuple[str, object]]) -> _Repeated:
    """Build every JSON object as a `_Repeated`, so a key written twice survives.

    Handed to `json.loads` as its `object_pairs_hook`, which is called once for every
    object in the document, so a repeat is caught at every layer: the top, the rules,
    the paths under a rule, and the keys of an entry.
    """
    return _Repeated(pairs)


def _refuse_repeats(value: object, where: str) -> None:
    """Refuse a list that wrote one key twice, naming the key and the layer it is in."""
    repeated = getattr(value, "repeated", None)
    if not repeated:
        return
    named = ", ".join(repr(key) for key in repeated)
    raise ValueError(
        f"{where} writes {named} twice, and only the last of the two would be read. "
        "A record written twice would go missing with nothing to say why, so a list "
        "that does it is refused rather than half read"
    )


def _entries(document: object) -> dict[str, dict[str, list[tuple[str, int | None]]]]:
    """The recorded entries, or a refusal naming what is wrong with the list.

    A path holds a list of entries rather than one entry, because one file can
    raise two findings under the same rule and a person who has read both has to be
    able to record both.

    A key written twice is refused like any other wrong shape. Two records for one
    path are two entries in a list and are read as two; the same key twice is one
    object claiming two things about one layer, and only the second of them is ever
    read, so the first would vanish without a word.

    The condition is normalised here and the claim a finding carries is normalised
    again where the two are compared. Both sides are normalised because the text in
    the list is a person copying an assertion out of a source file, and one spelling
    of an expression has to compare equal to another whatever either of them looked
    like. Two spellings that did not compare equal would report a correct entry as
    stale every run until its author changed it to the scanner's idea of it.

    Each entry needs a condition and a reason a reader can act on. A line is
    optional and only read when it is there, and a key this version does not know is
    left alone rather than refused, so a list can carry a note for its reader without
    this check having to learn the key first.

    A rule the check never reports is not left alone, because a note cannot name one
    without beginning with an underscore and an entry that can never be matched is
    not a note. It is refused with the same weight as any other wrong shape, so a typo
    in a rule id is a failure on every run rather than a record that quietly waives
    nothing.

    Anything else is refused rather than skipped, and this returns either every
    entry or none. Half a list waives half the findings it was written for and
    reports the rest as though nobody had recorded anything, which reads as a
    broken check rather than as a broken list.
    """
    if not isinstance(document, dict):
        raise ValueError("the list is not a JSON object")
    _refuse_repeats(document, "the list")
    recorded = document.get(WAIVERS_KEY)
    if not isinstance(recorded, dict):
        raise ValueError(f"{WAIVERS_KEY!r} is missing, or is not an object")
    _refuse_repeats(recorded, f"the entries under {WAIVERS_KEY!r}")
    out: dict[str, dict[str, list[tuple[str, int | None]]]] = {}
    for rule, by_path in recorded.items():
        if rule.startswith("_"):
            continue
        if rule not in RULE_IDS:
            # A key beginning with an underscore is a note for a reader and is skipped;
            # anything else is held to be a rule this check emits. An entry naming a rule
            # the check never reports is inert, and an inert entry is the worst kind: on a
            # file a run compares it is reported as stale, which a reader reads as a typo
            # they can fix, and on a file no run compares it is named and waives nothing,
            # so it looks like it is recording something.
            raise ValueError(
                f"{rule!r} is not a rule this check reports, so an entry naming it can "
                f"never be matched to a finding. The rules are "
                f"{', '.join(sorted(RULE_IDS))}"
            )
        if not isinstance(by_path, dict):
            raise ValueError(f"the entries for {rule} are not an object")
        _refuse_repeats(by_path, f"the entries for {rule}")
        paths: dict[str, list[tuple[str, int | None]]] = {}
        for path, leaves in by_path.items():
            if path.startswith("_"):
                continue
            where = f"the entries for {rule} in {path}"
            if not isinstance(leaves, list):
                raise ValueError(f"{where} are not a list")
            read: list[tuple[str, int | None]] = []
            for number, leaf in enumerate(leaves, start=1):
                one = f"entry {number} of {where}"
                if not isinstance(leaf, dict):
                    raise ValueError(f"{one} is not an object")
                _refuse_repeats(leaf, one)
                condition = leaf.get("condition")
                reason = leaf.get("reason")
                if not isinstance(condition, str) or not isinstance(reason, str):
                    raise ValueError(f"{one} needs a condition and a reason, both strings")
                if not reason.strip():
                    raise ValueError(f"{one} gives a reason that is empty")
                line = leaf.get("line")
                if line is not None and (isinstance(line, bool) or not isinstance(line, int)):
                    raise ValueError(f"{one} records a line that is not a number")
                read.append((normalise(condition), line))
            paths[path] = read
        out[rule] = paths
    return out


def apply_waivers(
    surface: Surface,
    recorded: dict[str, dict[str, list[tuple[str, int | None]]]],
    work_texts: dict[str, str],
) -> None:
    """Move every finding a person recorded as intended out of the failures.

    A finding is waived when its rule, its path and its condition all match an
    entry, and when a recorded line is a line that finding reports. Anything else
    about the entry is reported rather than passed over, and each of these fails
    the run:

    - an entry matching no finding in a file this run compared records a text this
      run does not hold as any finding's claim, and the file either no longer
      asserts that text at all or asserts it only on a line other than the
      recorded one;
    - an entry recording no line where more than one finding carries that claim,
      because it cannot say which of them its author read;
    - a recorded line that no finding carrying that claim is reported on, so the
      entry has slid onto a different assertion;
    - a second finding that could only have been silenced by the same entry as an
      earlier one, because the entry was taken.

    The first of those has one exception, and it is the normal end of an entry's
    life: findings exist only while a condition differs from the base, so the
    moment the change a record waived has merged, no finding carries the claim
    any more. When the recorded claim is still one of the compared file's own
    assertions sitting on the line the entry records — or on any line, for an
    entry that recorded none — the entry is spent rather than stale: reported,
    never a failure, and free to be deleted.

    One entry silences one finding, and the recorded line is what says which. Two
    findings in one file under one rule can carry the same claim, and nothing but
    their lines tells them apart, so an entry is matched against the candidates
    sitting on the line it recorded rather than against the first one the scan
    produced. Reading the pool in order instead would refuse every record that
    named the second of two identical findings and report it as naming an
    assertion its author never read.

    An entry that recorded no line takes the only candidate there is, which is the
    case where nothing was ambiguous. More than one candidate with no line to
    choose between is refused instead: taking the first would waive a finding on a
    guess, and the guess would be invisible, because the run would then be green on
    a judgement about an assertion its author may never have read.

    Failing these is the safe direction. Removing an entry puts the finding back
    under the check and the next run says so. Adding an entry that is wrong turns
    a working check into a list of things it no longer says, and the run goes
    green either way, so nothing else in the tree would ever report it.

    An entry for a file this run never compared is reported and judged by
    nothing. A clean tree compares no file at all, so failing a run over a record
    for a file it never opened would be a red run with no cause behind it.
    """
    waived: list[Finding] = []
    taken: set[int] = set()
    for rule, by_path in recorded.items():
        for path, entries in by_path.items():
            for condition, line in entries:
                matched = [
                    finding
                    for finding in surface.findings
                    if id(finding) not in taken
                    and finding.rule == rule
                    and finding.path == path
                    # Both sides normalised, because a claim is the scanner's own
                    # spelling of an assertion and is not always the spelling the
                    # reader would write. The claim of a comparing macro is two
                    # halves already normalised and then joined with a spaced `==`,
                    # and that joined text was never normalised as a whole, so it
                    # kept a space `normalise` drops and could never compare equal
                    # to a condition read out of the file. The claim of an `assert!`
                    # is one normalised expression already, so normalising it changes
                    # nothing, and the asymmetry is deliberate: leave it out and every
                    # entry for `assert_eq!` is stale on every run.
                    and normalise(finding.claim) == condition
                ]
                if path not in surface.scanned:
                    # The rule and the path, not a sentence. One line is printed for
                    # the lot and names these, so a list holding two records for one
                    # path cannot print the same line twice.
                    surface.unexercised_waivers.append(f"{rule} in {path}")
                elif not matched:
                    work_text = work_texts.get(path)
                    # A claim some finding this run still carries — under this
                    # rule or another, and whether an earlier entry took it or
                    # not — means the condition still differs from the base, so
                    # nothing has landed. That entry is wrong rather than spent,
                    # and it stays a failure below.
                    carried = any(
                        normalise(finding.claim) == condition
                        for finding in surface.findings
                    )
                    spent = False
                    slid: list[int] = []
                    if work_text is not None and not carried:
                        # The claim is built the way `compare_file` builds
                        # `Finding.claim` -- the assertion's own condition -- and
                        # normalised on the same side as the recorded condition,
                        # so a comparing macro's spaced `==` halves still meet the
                        # entry. The raw file text is never searched: a record is
                        # written over several lines and only the parsed,
                        # normalised form can match it. The line decides here as
                        # it does everywhere else in this function: the recorded
                        # line is what says which assertion the author read, so
                        # the claim only spends the entry where the entry says it
                        # read it. An entry with no recorded line has no line to
                        # check, so presence anywhere spends it. A claim the file
                        # holds only elsewhere has slid, and that stays a failure
                        # naming both lines.
                        for group in assertions_in(work_text).values():
                            for one in group:
                                if normalise(one.condition) != condition:
                                    continue
                                if line is None or one.line == line:
                                    spent = True
                                else:
                                    slid.append(one.line)
                    if spent:
                        surface.spent_waivers.append(
                            f"{rule} in {path}: the recorded claim `{condition}` is one of "
                            "the file's own assertions, so the change it waived has landed; "
                            "the entry no longer judges anything and can be deleted"
                        )
                    elif slid:
                        seen = ", ".join(str(number) for number in sorted(set(slid)))
                        surface.stale_waivers.append(
                            f"{rule} in {path}: the recorded claim `{condition}` sits on line "
                            f"{seen} of this file, not the recorded line {line}; the change it "
                            "waived has landed, so delete the entry or record the line it sits on"
                        )
                    else:
                        surface.stale_waivers.append(
                            f"{rule} in {path}: this run compared the file and no finding carries "
                            f"the recorded claim `{condition}`, so the recorded text is not the "
                            "claim this run holds. Every finding's line prints the claim a record "
                            "has to hold"
                        )
                elif line is not None:
                    # The line decides. The candidates are the findings this entry could
                    # match, and the one sitting on the recorded line is the one its author
                    # read, so an entry naming the second of two identical findings waives
                    # the second and leaves the first to be reported.
                    on_line = [finding for finding in matched if finding.line == line]
                    if not on_line:
                        seen = ", ".join(str(finding.line) for finding in matched)
                        surface.stale_waivers.append(
                            f"{rule} in {path}: the entry records the claim `{condition}` on "
                            f"line {line}, and this run reports that claim on "
                            f"{'line' if len(matched) == 1 else 'lines'} {seen}. Copy the line "
                            "out of the line the finding printed"
                        )
                    else:
                        waived.append(on_line[0])
                        taken.add(id(on_line[0]))
                elif len(matched) > 1:
                    surface.stale_waivers.append(
                        f"{rule} in {path}: the entry records the claim `{condition}` and no "
                        f"line, and this run reports that claim on {len(matched)} lines: "
                        f"{', '.join(str(finding.line) for finding in matched)}. An entry with "
                        "no line cannot say which one was read, so record one entry per line"
                    )
                else:
                    waived.append(matched[0])
                    taken.add(id(matched[0]))
    # `id` rather than `==` because a finding carries no comparison of its own, so
    # identity is the only test here that means the same object, and every finding
    # in the list is one object rather than a copy of another.
    waived_ids = {id(finding) for finding in waived}
    surface.waived.extend(finding for finding in surface.findings if id(finding) in waived_ids)
    surface.findings = [
        finding for finding in surface.findings if id(finding) not in waived_ids
    ]


def compare_file(
    relative: str,
    base_text: str | None,
    work_text: str,
    surface: Surface,
) -> None:
    """Judge one file against its base copy."""
    if base_text is None:
        # A file the base did not have, or one this run deleted, makes no claim
        # about an assertion that used to exist. The compiler and the test suite
        # own it.
        return
    for key in sorted(test_functions(base_text) - test_functions(work_text)):
        surface.removed_tests.append(
            f"{relative}: the test function `{key}` was at the base and is not in "
            "this tree now, so it was removed, renamed, or moved to another file. "
            "No rule here can follow a rename, so read the change and sign off on "
            "it, or restore the function"
        )
    before = assertions_in(base_text)
    after = assertions_in(work_text)
    for key, assertions in after.items():
        original = before.get(key)
        if original is None:
            # A function the base did not hold, or one that moved house. Either
            # way its assertions are not matched to anything, and guessing which
            # base assertion each one replaced would be a rule that invents claims.
            continue
        by_kind: dict[str, list[Assertion]] = {}
        for assertion in original:
            by_kind.setdefault(assertion.macro, []).append(assertion)
        # Iterate the kinds the base held as well as the kinds held now. Walking
        # only what is present now made a removal invisible: a function whose last
        # `assert!` was deleted has no `assert!` left to group, so the kind never
        # appeared and the count comparison below never ran for it. Deleting the
        # only assertion a function had was the one thing this rule most had to
        # catch.
        kinds = list(dict.fromkeys([*by_kind, *(macro for macro, _ in by_kind_of(assertions))]))
        for macro in kinds:
            current = dict(by_kind_of(assertions)).get(macro, [])
            was = by_kind.get(macro, [])
            if not was:
                for assertion in current:
                    surface.findings.append(
                        Finding(
                            relative,
                            assertion.line,
                            "R4-added",
                            f"a new `{macro}!` in `{key}`, which held none of that "
                            f"kind at the base. It asserts `{assertion.condition}`, and "
                            "no assertion in that function asserted it at the base. "
                            "Adding an assertion where the base had none is how an "
                            "invented assertion gets in, because nothing about it was "
                            "ever reviewed",
                            claim=assertion.condition,
                        )
                    )
                continue
            if len(current) < len(was):
                # No single assertion is missing here, so the claim names the whole
                # change instead: the function, the kind, and the count on each side of
                # it. The same text goes in the message, because a record has to hold
                # it and the counts in the prose are not a spelling anybody could copy.
                claim = f"{key} {macro} {len(was)}->{len(current)}"
                surface.findings.append(
                    Finding(
                        relative,
                        0,
                        "R1-removed",
                        f"`{key}` held {len(was)} `{macro}!` at the base and holds "
                        f"{len(current)} now, so {len(was) - len(current)} of them "
                        f"were removed. The whole change is `{claim}`. The count cannot "
                        "fall for a change that was meant only to add a message",
                        claim=claim,
                    )
                )
            for now_one, was_one in _pair_up(was, current):
                if was_one is None:
                    surface.findings.append(
                        Finding(
                            relative,
                            now_one.line,
                            "R1-changed",
                            f"this `{macro}!` asserts `{now_one.condition}` and no "
                            f"assertion in `{key}` asserted that at the base, so it is "
                            "new rather than rewritten. An assertion nothing reviewed "
                            "is how an invented one gets in",
                            claim=now_one.condition,
                        )
                    )
                elif not _same_claim(was_one, now_one):
                    surface.findings.append(
                        Finding(
                            relative,
                            now_one.line,
                            "R1-changed",
                            f"this `{macro}!` asserted `{was_one.condition}` and now "
                            f"asserts `{now_one.condition}`. Adding a message is the "
                            "allowed change; changing what is asserted is not, and it "
                            "is invisible in a diff that only adds text",
                            claim=now_one.condition,
                        )
                    )
                # The message is judged whether or not the condition changed. An
                # earlier version stopped here on a condition finding, which hid
                # every message problem behind the first condition that moved, so
                # one report stood for several defects.
                #
                # The base message is the one for the assertion this one was
                # matched to, and None when nothing matched, which means the
                # message is new and is judged in full.
                if was_one is not None and now_one.message == was_one.message:
                    continue
                for rule, detail in message_problems(now_one.message):
                    surface.findings.append(
                        Finding(
                            relative,
                            now_one.line,
                            rule,
                            detail,
                            # The message belongs to one assertion, so a problem in
                            # it is about that assertion. The claim is passed here
                            # rather than returned with the pair, because the pair
                            # says what is wrong with a message and not which
                            # assertion the message belongs to.
                            claim=now_one.condition,
                        )
                    )


def _pair_up(
    was: list[Assertion], current: list[Assertion]
) -> list[tuple[Assertion, Assertion | None]]:
    """Match each working assertion to the base assertion it replaced, if any.

    Matched by what they assert, not by where they sit. Binding a value to a `let`
    and asserting on the name moves an assertion's text away from the base's, and a
    mechanical pass that also grouped neighbouring assertions reordered them; a
    positional match then compared each assertion against a different one and
    reported a faithful rewrite as a changed claim. Nine of them did exactly that
    on a real tree, every one of them a false finding.

    Two passes, and the order matters. An assertion whose claim matches a base one
    is matched to it first, so a claim that survived is not also offered as the
    comparison for something else. Whatever is left is then paired in written
    order, and a working assertion with nothing left to pair against asserts
    something the base never asserted.

    A base assertion already matched is not reused, so two assertions asserting the
    same thing are matched to two base assertions rather than to one.
    """
    unmatched = list(was)
    paired: list[tuple[Assertion, Assertion | None]] = []
    for now_one in current:
        found = next(
            (
                index
                for index, was_one in enumerate(unmatched)
                if _same_claim(was_one, now_one)
            ),
            None,
        )
        if found is not None:
            paired.append((now_one, unmatched.pop(found)))
        else:
            paired.append((now_one, None))
    spare = list(unmatched)
    for position, (now_one, was_one) in enumerate(paired):
        if was_one is None and spare:
            paired[position] = (now_one, spare.pop(0))
    return paired


def _same_claim(was_one: Assertion, now_one: Assertion) -> bool:
    """Whether two assertions still assert the same thing.

    Equal as written is the ordinary answer, and it is checked first because the
    expansion below is not stable: adding an unrelated binding in the same
    function changes what an unrelated condition expands to, so two assertions
    whose text never moved must not be compared through it.
    """
    if was_one.condition == now_one.condition:
        return True
    # Otherwise one side may name a value the other side calls. Putting the
    # bindings back on both is what makes that comparable, and a condition that
    # still differs afterwards is a real change.
    return was_one.expanded == now_one.expanded or (
        was_one.condition == now_one.expanded or was_one.expanded == now_one.condition
    )


def by_kind_of(assertions: list[Assertion]) -> list[tuple[str, list[Assertion]]]:
    """Assertions grouped by macro, in the order the macros first appear."""
    order: list[str] = []
    grouped: dict[str, list[Assertion]] = {}
    for assertion in assertions:
        if assertion.macro not in grouped:
            order.append(assertion.macro)
            grouped[assertion.macro] = []
        grouped[assertion.macro].append(assertion)
    return [(macro, grouped[macro]) for macro in order]


def scan(root: Path, base: str, staged: bool = False) -> Surface:
    """Judge every Rust file this run has to compare, and return the surface."""
    surface = Surface()
    resolved = resolve_base(root, base)
    if resolved is None:
        surface.unreadable.append(
            f"base `{base}` does not resolve, and `origin/{base}` does not either. "
            "Fetch it, or pass --base with a ref this checkout has"
        )
        return surface
    if staged:
        base_sha = "HEAD"
    else:
        status, output = _git(root, "merge-base", "HEAD", resolved)
        base_sha = output.strip()
        if status != 0 or not base_sha:
            surface.unreadable.append(
                f"`HEAD` has no merge base with `{resolved}`, so there is nothing to "
                "compare against. A branch with no shared history is a state, not a "
                "clean run"
            )
            return surface

    work_texts: dict[str, str] = {}
    for relative in changed_files(root, base_sha, staged):
        surface.scanned.append(relative)
        work_text = read_work(root, relative, staged, surface)
        if work_text is None:
            if not staged:
                base_text = read_base(root, base_sha, relative, surface)
                if base_text is not None and not (root / relative).exists():
                    surface.unreadable = [
                        entry
                        for entry in surface.unreadable
                        if not entry.startswith(f"{relative}:")
                    ]
                    continue
            continue
        # Kept so an entry that matched nothing can be told apart from one whose
        # claim the file still asserts. A path whose text could not be read is
        # left out, and its entries keep today's stale answer.
        work_texts[relative] = work_text
        base_text = read_base(root, base_sha, relative, surface)
        if base_text is None and not staged:
            # Not an error: a new file has no base copy.
            continue
        compare_file(relative, base_text, work_text, surface)
    # Last, so that everything reading `exit_code` gets the same answer: the
    # judgements are read against the findings the whole scan produced, not
    # against a file's findings as that file was read.
    apply_waivers(surface, read_waivers(root, staged, surface), work_texts)
    return surface


def not_exercised_line(records: list[str], compared: list[str]) -> str:
    """The one line a run prints about the records it did not judge.

    One line rather than one per record. The list grows one entry per judgement, and a
    line per entry is printed on every unrelated commit and on every build, which
    trains a reader to skip the output that also carries the one line that matters.
    Two records for one path read as two identical lines, so the number of lines told
    a reader nothing they could act on.

    The line prints when the count is above zero and not otherwise, so nothing was
    skipped and something was not judged cannot be read as the same run. The count is
    printed as well as the names, and so is the number of names the cap left out, so a
    short line is a short line rather than a line a reader has to trust.

    The paths the run did compare go on the same line, because a record named for a
    path the run did not open is nearly always a path spelled wrongly, and the reader
    cannot tell those two apart from the line alone. The run compares no file at all on
    a clean tree, and saying so is better than naming a path that does not exist.
    """
    count = len(records)
    one = count == 1
    sentence = (
        f"{count} recorded {'entry is' if one else 'entries are'} for a path that is "
        "not one of the paths this run compared, so nothing here can say whether "
        f"{'it is' if one else 'they are'} still needed"
    )
    shown = records[:NOT_EXERCISED_SHOWN]
    named = ", ".join(shown)
    if count > len(shown):
        named += f", and {count - len(shown)} more not shown"
    if not compared:
        seen = (
            "and this run compared no file at all, so there is nothing here to match "
            "them against"
        )
    else:
        seen = f"and this run compared {', '.join(compared[:COMPARED_SHOWN])}"
        if len(compared) > COMPARED_SHOWN:
            seen += f", and {len(compared) - COMPARED_SHOWN} more not shown"
    return f"{sentence}: {named}; {seen}"


def report(root: Path, base: str, staged: bool) -> int:
    """Run the check and print its verdict."""
    surface = scan(root, base, staged)
    for path in surface.unreadable:
        print(f"{NAME}: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"{NAME}: FINDING {finding}")
    for finding in surface.waived:
        print(f"{NAME}: WAIVED {finding}")
    for entry in surface.stale_waivers:
        print(f"{NAME}: STALE WAIVER {entry}")
    for entry in surface.spent_waivers:
        print(f"{NAME}: SPENT WAIVER {entry}")
    for entry in surface.removed_tests:
        print(f"{NAME}: REMOVED TEST {entry}")
    if surface.unexercised_waivers:
        # One line, always while the count is above zero, and the count is on it.
        said = not_exercised_line(surface.unexercised_waivers, surface.scanned)
        print(f"{NAME}: WAIVER NOT EXERCISED {said}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        # The recorded list is counted out of the paths and given its own sentence. What
        # is missing there is not a path this run failed to open, it is the record of
        # every judgement anybody made, and a sentence about paths sends the reader to
        # the wrong cause. The count of files compared is on it because the run did read
        # the tree and a reader who cannot see that thinks nothing was judged at all.
        gone = [said for said in surface.unreadable if WAIVERS_PATH in said]
        if gone:
            print(
                f"{NAME}: the recorded list could not be read {len(gone)} time(s), so no "
                f"judgement anyone recorded was honoured, and this run compared "
                f"{len(surface.scanned)} file(s) without it",
                file=sys.stderr,
            )
        if len(surface.unreadable) - len(gone):
            print(
                f"{NAME}: {len(surface.unreadable) - len(gone)} of the things this check had "
                "to read could not be read, and the rest of this run does not cover them",
                file=sys.stderr,
            )
        return code
    if code == EXIT_FINDINGS:
        counted = [f"{len(surface.findings)} finding(s)"]
        if surface.waived:
            counted.append(f"{len(surface.waived)} waived")
        if surface.stale_waivers:
            # Counted here and not only on its own line above, so a reader who
            # reads the last line alone still learns that something is wrong with
            # the recorded list rather than with the assertions.
            counted.append(f"{len(surface.stale_waivers)} stale waiver(s)")
        print(f"{NAME}: {', '.join(counted)}")
        return code
    if surface.waived:
        # The recorded judgements go inside the sentence rather than after it. The
        # old sentence claims every assertion still asserts the same thing, which is
        # false on a run where one no longer does, and a reader who reads only the
        # last line of a run is told so. The count is still appended, because a
        # reader scanning the end of a long log should find it there.
        verdict = (
            f"{NAME}: every assertion compared still asserts the same thing except the "
            f"{len(surface.waived)} recorded above, and every message a change added is "
            f"well formed ({len(surface.scanned)} file(s) compared)"
        )
    else:
        # Word for word what a clean run has always printed. A hook and a reader of
        # its output both know this line, and nothing recorded here is worth changing
        # it for.
        verdict = (
            f"{NAME}: every assertion still asserts the same thing, and every message a "
            f"change added is well formed ({len(surface.scanned)} file(s) compared)"
        )
    spared = []
    if surface.waived:
        spared.append(f"{len(surface.waived)} waived")
    if surface.unexercised_waivers:
        spared.append(f"{len(surface.unexercised_waivers)} for a file not compared")
    if surface.removed_tests:
        spared.append(
            f"{len(surface.removed_tests)} removed or renamed test function(s) to sign off"
        )
    if spared:
        verdict += f", {', '.join(spared)}"
    print(verdict)
    return code


# --- Self-test ------------------------------------------------------------------
#
# Every case is a throwaway git repository holding a base commit and a working
# tree that differs from it, so a case describes the check rather than this
# checkout and gives the same answer on any machine. The repositories carry their
# own identity and turn signing off, because a throwaway repository otherwise
# reads the machine's git configuration and a suite that inherits that answers
# differently on a build machine than on a laptop.

CLEAN_BASE = """mod fake {
    struct Fake {
        budget: u32,
    }

    impl Fake {
        fn hide(&mut self) -> bool {
            self.budget = self.budget.saturating_sub(1);
            self.budget > 0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn spends_one_call() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed");
            assert!(!fake.hide(), "a spent budget must not hide");
        }
    }
}
"""


#: A function holding exactly one assertion, of a kind nothing else in the base
#: uses. Deleting it leaves the function with no assertion at all, so the kind
#: disappears from the file rather than merely shrinking.
ONLY_ONE_EQ = """

#[cfg(test)]
mod only_one {
    #[test]
    fn compares_a_single_value() {
        assert_eq!(1 + 1, 2);
    }
}
"""


def _git_in(root: Path, *args: str) -> None:
    """Run a git command in a fixture repository, ignoring failure."""
    subprocess.run(  # noqa: S603
        ["git", *args],  # noqa: S607
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )


FIXTURE_PATH = "crates/demo/src/fake.rs"

#: The recorded list every fixture repository carries. It is written before the
#: first commit so that the index holds it as well as the disk, because staged
#: mode reads the list from the index and a case judged against a list that only
#: exists on disk would be judged against nothing.
NO_WAIVERS = """{
  "_why": "Nothing has been recorded as intended in this repository.",
  "_waivers": {}
}
"""


def _write_waivers(root: Path, text: str) -> None:
    """Put the recorded list in the place the check looks for it."""
    path = root / WAIVERS_PATH
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _fixture(
    base: Path,
    label: str,
    work: str | None = None,
    base_text: str = CLEAN_BASE,
    staged: bool = False,
) -> Path:
    """One case: a repository whose base commit holds `base_text`, then a change.

    The base and the working tree are separate parameters because the negative
    control for one rule is often the positive case for another. A control that
    had to be built by editing the working tree alone would change the base as
    well, and then the pair would no longer differ only in the way under test.

    In staged mode the defect is put in the index and the working tree is put
    back to the base, so a run that reads the disk instead of the index reports
    nothing and the case fails.

    Every repository starts with a list holding nothing recorded. A case that
    needs an entry writes one afterwards, because an entry that has to agree with
    a line can only be written once there is a working tree to read the line from.
    """
    root = base / label
    target = root / FIXTURE_PATH
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(base_text, encoding="utf-8")
    _write_waivers(root, NO_WAIVERS)
    _git_in(root, "init", "--quiet")
    _git_in(root, "config", "user.email", "self-test@example.invalid")
    _git_in(root, "config", "user.name", "self-test")
    _git_in(root, "config", "commit.gpgsign", "false")
    _git_in(root, "add", "-A")
    _git_in(root, "commit", "--quiet", "-m", "base")
    if work is not None:
        target.write_text(work, encoding="utf-8")
        if staged:
            _git_in(root, "add", "-A")
            target.write_text(base_text, encoding="utf-8")
    return root


#: The base with a message added that names a local, which is the shape a
#: message takes when the value is already bound. This is the clean half of every
#: message case: the same assertion, the same message position, nothing called
#: and nothing inlined wrongly.
MESSAGED = CLEAN_BASE.replace(
    'assert!(fake.hide(), "the first hide must succeed");',
    'assert!(fake.hide(), "the first hide must succeed: {budget:?}");',
)

#: The base of the "bound to a let first" pair. The call is bound here rather
#: than in the working tree, because binding it in the working tree would change
#: the condition from `fake.hide()` to `first`, and rule 1 would fire on the
#: control before rule 2 could be said to hold.
BOUND_BASE = CLEAN_BASE.replace(
    "            let mut fake = Fake { budget: 1 };",
    "            let mut fake = Fake { budget: 1 };\n            let first = fake.hide();",
).replace(
    'assert!(fake.hide(), "the first hide must succeed");',
    'assert!(first, "the first hide must succeed");',
)

#: A base and a working tree for the chain case. The base already binds the call,
#: and the working tree adds a second binding around it, which is how this is done
#: when a value needs a name of its own. Each link of the chain is a name inside
#: the previous one's argument list, so a check that resolves only the outermost
#: name reports a faithful rewrite as a changed claim.
CHAIN_BASE = """mod chain {
    struct Fake {
        budget: u32,
    }

    impl Fake {
        fn hide(&mut self) -> bool {
            self.budget = self.budget.saturating_sub(1);
            self.budget > 0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn same(value: &bool, label: &str) -> bool {
            let _ = label;
            *value
        }

        #[test]
        fn one_call_is_allowed() {
            let mut fake = Fake { budget: 1 };
            let first = same(&fake.hide(), "one call");
            assert!(first, "the first hide must succeed");
        }
    }
}
"""

#: The base already binds a call inside a call, so the working tree only has to
#: add a message. The chain runs through both bindings, and the string passed to
#: the inner one has to survive it.
CHAIN_WORK = CHAIN_BASE.replace(
    '            assert!(first, "the first hide must succeed");',
    '            assert!(first, "the first hide must succeed: {first:?}");',
)

#: The same chain, but with a string passed down two links. It is here because an
#: earlier version resolved names inside brackets and read the binding's own
#: arguments as the assertion's, which turned `expect("persona doc")` into
#: `expect()` and reported nine honest rewrites as changed conditions.
CHAIN_LITERAL_BASE = CHAIN_BASE.replace(
    'let first = same(&fake.hide(), "one call");',
    'let first = same(&fake.hide(), "persona doc");',
)

CHAIN_LITERAL_WORK = CHAIN_LITERAL_BASE.replace(
    '            assert!(named, "the first hide must succeed");',
    '            assert!(named, "the first hide must succeed: {named:?}");',
)

#: A base whose only feature is a method holding no assertion at all. Rule 4 is
#: about a function that had none of a kind, so a case for it needs one that had
#: none, and this is that function. It is the historical defect's own shape: a
#: budget of one call, and an assertion invented inside the code that spends it.
NO_ASSERT_BASE = CLEAN_BASE.replace(
    "        fn hide(&mut self) -> bool {\n"
    "            self.budget = self.budget.saturating_sub(1);\n"
    "            self.budget > 0\n"
    "        }",
    "        fn hide(&mut self) -> bool {\n"
    "            let hid = self.budget > 0;\n"
    "            self.budget = self.budget.saturating_sub(1);\n"
    "            hid\n"
    "        }",
)


#: A whole file the base did not hold. Written out rather than derived, because
#: a file that has to be built by editing another one is a file whose assertions
#: may not be the ones the case names.
NEW_FILE = """mod extra {
    #[cfg(test)]
    mod tests {
        #[test]
        fn a_new_test() {
            let rows = Vec::<u32>::new();
            assert!(rows.is_empty(), "{rows:?}");
        }
    }
}
"""


def _new_file_case(base: Path) -> Path:
    """A case whose only change is a file the base did not have."""
    root = _fixture(base, "r4-new-file")
    extra = root / "crates/demo/src/extra.rs"
    extra.write_text(NEW_FILE, encoding="utf-8")
    return root


#: The base with its one test function taken out whole. A function that is gone
#: leaves no assertion behind to group, so no rule here fires on it: the
#: assertions went with the function and nothing is left to compare. That is the
#: blind spot this case pins -- the run stays green, and the report line is the
#: only thing a reader gets.
REMOVED_TEST_WORK = CLEAN_BASE.replace(
    "        #[test]\n"
    "        fn spends_one_call() {\n"
    '            let mut fake = Fake { budget: 1 };\n'
    '            assert!(fake.hide(), "the first hide must succeed");\n'
    '            assert!(!fake.hide(), "a spent budget must not hide");\n'
    "        }\n",
    "",
)


def _removed_test_case(base: Path) -> Path:
    """A case whose one test function is gone from the working tree."""
    return _fixture(base, "fn-removed", REMOVED_TEST_WORK)


#: Spellings that must all normalise to the same thing, because a rewrite that
#: reflowed a line changed nothing it asserted. Each pair here was a false finding
#: on a real tree before it became a case.
NORMALISE_PAIRS = (
    ("store.list(&bank,10).unwrap().is_empty()", "store .list ( &bank , 10 ) .unwrap ( ) .is_empty ( )"),
    ("RankList{weight:1.0,scores:None}", "RankList { weight: 1.0 , scores: None , }"),
    ("detect_noise(\"clean lesson: cache before query\").k.is_empty()",
     "detect_noise( \"clean lesson: cache before query\" ).k.is_empty()"),
    ('f("a, b",c)', 'f( "a, b" ,c )'),
    # A doubled space inside a literal is a difference, not a spelling. It stays a
    # difference here so the rule cannot grow to flatten a value.
    ("rows.len()", "rows .len ( )"),
    ("a is b", "a  is  b"),
    ("rows[0].len() == 0", "rows[ 0 ] .len ( ) == 0"),
)


def _normalise_case(base: Path) -> tuple[str, bool, Path]:
    """Assert that each pair of spellings collapses to one, with no git involved."""
    failures = [
        (left, right, normalise(left), normalise(right))
        for left, right in NORMALISE_PAIRS
        if normalise(left) != normalise(right)
    ]
    ok = not failures
    detail = "; ".join(
        f"{left} -> {mine!r} but {right} -> {theirs!r}"
        for left, right, mine, theirs in failures
    )
    print(
        f"self-test {'ok' if ok else 'FAIL'}: {len(NORMALISE_PAIRS)} spelling pairs normalise "
        f"to one form ({detail or 'all pairs agree'})"
    )
    # The other direction, and it matters as much: a difference inside a literal is
    # a difference in the value, so flattening it would hide a real change. An
    # earlier version did exactly that, turning a message's spacing into a claim
    # about code.
    differs = normalise('assert_eq!(f("a, b"), x)') != normalise('assert_eq!(f("a,b"), x)')
    print(
        f"self-test {'ok' if differs else 'FAIL'}: spacing inside a literal is still a "
        "difference, so the rule cannot flatten a value"
    )
    del base
    return ok and differs


#: A base whose one assertion is on a number, so the condition a waiver names is a
#: line of text a person copies out of the file rather than a call they have to
#: read twice to spell.
BUDGET_BASE = CLEAN_BASE.replace(
    "            let mut fake = Fake { budget: 1 };\n",
    "            let budget = 2;\n            let mut fake = Fake { budget };\n",
).replace(
    '            assert!(fake.hide(), "the first hide must succeed");',
    '            assert!(budget == 2, "a budget of two: {budget:?}");',
)

BUDGET_WORK = BUDGET_BASE.replace("budget == 2", "budget == 3")

#: The list a person writes for that finding. It is written as a person writes one
#: rather than through `json.dumps`, so the case reads the file a reader of the
#: repository would read, and the line it records is filled in from the working
#: tree below: an entry whose line disagrees with its finding fails the run, and a
#: hand-counted one would rot the first time a fixture moved.
WAIVED_BUDGET = """{
  "_why": "The budget a fake is given is a decision, not an accident.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the assertion names the budget, because a spent budget must not hide",
          "line": 0
        }
      ]
    }
  }
}
"""


def _waived_budget_case(base: Path) -> Path:
    """A case whose one finding a person has read and then recorded."""
    root = _fixture(base, "waiver-matched", BUDGET_WORK, base_text=BUDGET_BASE)
    line = _expected_lines(root, {"R1-changed": "budget == 3"})["R1-changed"]
    _write_waivers(root, WAIVED_BUDGET.replace('"line": 0', f'"line": {line}'))
    return root


#: A second file, which this case's run will not compare because nothing in it
#: changed, so the list has something to name that the run cannot judge.
UNTOUCHED = """mod untouched {
    pub fn budget() -> u32 {
        2
    }
}
"""

UNEXERCISED_WAIVERS = """{
  "_why": "A judgement recorded while the tree stood still.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/untouched.rs": [
        {
          "condition": "budget() == 3",
          "reason": "the third call is the one that must fail",
          "line": 3
        }
      ]
    }
  }
}
"""


def _unexercised_case(base: Path) -> Path:
    """A case whose recorded entry names a file the run does not compare."""
    root = _fixture(base, "waiver-unexercised", MESSAGED)
    other = root / "crates/demo/src/untouched.rs"
    other.write_text(UNTOUCHED, encoding="utf-8")
    # Only that one path is added. Adding the lot would take the changed assertion
    # into the same commit, and the run would then have nothing left to compare.
    _git_in(root, "add", "crates/demo/src/untouched.rs")
    _git_in(root, "commit", "--quiet", "-m", "a file the cases leave alone")
    _write_waivers(root, UNEXERCISED_WAIVERS)
    return root


# What the recorded list does when it is absent, malformed, or records nothing a
# reader could act on, and what it does with an entry that no longer describes the
# tree. Every case above is a run that reports something; these are the runs whose
# answer is no, and they are here because a mechanism is judged by what it refuses.
# A rule that quietly stopped refusing is indistinguishable from a rule that has
# nothing left to do, so each refusal below is pinned by a case that goes red when
# the refusal goes.

#: A list in the wrong shape: the entries are a list, where the reader expects an
#: object of rules. This is the mistake a person makes by hand after writing a
#: single entry and forgetting the layer above it, and it is why the reader refuses
#: the whole list rather than reading the part it recognises.
LIST_AS_WAIVERS = """{
  "_why": "One entry, written without the layer the reader looks in.",
  "_waivers": []
}
"""

#: A list whose one entry records no reason. The condition is real and matches the
#: finding, so this list would waive it, and a reader would be left with a decision
#: and nothing said about it. The reason is a space rather than nothing at all,
#: because a run that only tested for an empty string would accept this one.
NO_REASON_WAIVERS = """{
  "_why": "A judgement with nothing written beside it.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "   "
        }
      ]
    }
  }
}
"""

#: A list whose one entry gives a number where the reason should be. Nothing reads a
#: reason as text, so the mistake looks harmless, and a reader that tests the type and
#: then treats the value as text crashes instead of refusing: the whole run ends in a
#: traceback and the person who wrote the entry learns nothing but the traceback.
NUMBERED_REASON_WAIVERS = """{
  "_why": "A judgement with the reason written as a number.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": 42,
          "line": LINE
        }
      ]
    }
  }
}
"""

#: A list filed under a rule id the check never reports, which is one letter typed twice
#: and nothing else. Both readings of an entry that can never match let it sit there
#: looking like it records something: on a file the run compares it is reported as
#: stale, which reads as a spelling to fix, and on a file no run compares it is named
#: and waives nothing at all.
UNKNOWN_RULE_WAIVERS = """{
  "_why": "One judgement filed under a rule with a letter too many.",
  "_waivers": {
    "R1-changedd": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the assertion says so",
          "line": LINE
        }
      ]
    }
  }
}
"""

#: The same record filed under the rule that removes an assertion rather than the one
#: that changes a condition. Both ids begin with the same two characters, so anything
#: comparing less than the whole id would let this silence a finding of the other kind,
#: and a record that waives a finding nobody matched is the one thing this list cannot
#: be used for.
WRONG_RULE_WAIVERS = """{
  "_why": "One judgement filed under the rule beside the right one.",
  "_waivers": {
    "R1-removed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the assertion says so",
          "line": LINE
        }
      ]
    }
  }
}
"""

#: A list whose entry names a condition nothing asserts, which is what a list looks
#: like after the assertion it was written for has been rewritten. Nothing about the
#: entry is malformed, so only the run against the tree can say it has stopped
#: describing one.
STALE_WAIVERS = """{
  "_why": "A judgement written for a version of the assertion that is gone.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 9",
          "reason": "the budget was three at the time this was written"
        }
      ]
    }
  }
}
"""

#: A list whose entry records the claim the file still asserts on the line the
#: entry records, which is the normal end of an entry's life: the change it
#: waived has landed, so no finding carries the claim any more, and the assertion
#: itself is still there. That is what tells a landed entry from one whose
#: assertion is gone. The line is filled in from the working tree below, because
#: the spent rule reads the recorded line and a hand-counted one would rot the
#: first time a fixture moved.
SPENT_WAIVERS = """{
  "_why": "A judgement whose change has landed in the base.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 2",
          "reason": "the budget of two is what the fake is given on purpose",
          "line": LINE
        }
      ]
    }
  }
}
"""

#: The same work as the landed case, with a different assertion changed: the
#: recorded claim sits in the file untouched, while a second assertion is the
#: one that differs from the base. The entry then records a line beside the
#: claim rather than its own, which is the slid shape.
SLID_WORK = BUDGET_BASE.replace(
    'assert!(!fake.hide(), "a spent budget must not hide");',
    'assert!(fake.hide(), "a spent budget must not hide");',
)

#: The same judgement filed against the line beside the claim. The claim is in
#: the file, so only the line says anything, and it says the record is wrong.
SLID_LANDED_WAIVERS = """{
  "_why": "A judgement whose recorded line no longer matches where the claim sits.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 2",
          "reason": "the budget of two is what the fake is given on purpose",
          "line": LINE
        }
      ]
    }
  }
}
"""

#: Two functions in one file asserting the same thing, so one record written for
#: either of them could be read as a record for both. Nothing but the line tells
#: the two findings apart: same rule, same file, same claim. That is why a record
#: for one of them has to name its line, and why an entry recording no line is
#: refused rather than given the first of them.
TWIN_BASE = """mod twin {
    #[cfg(test)]
    mod tests {
        #[test]
        fn first_budget() {
            let budget = 2;
            assert!(budget == 2, "a budget of two: {budget:?}");
        }

        #[test]
        fn second_budget() {
            let budget = 2;
            assert!(budget == 2, "a budget of two: {budget:?}");
        }
    }
}
"""

TWIN_WORK = TWIN_BASE.replace("budget == 2", "budget == 3")

#: The text both of the twin's findings carry, and the text a record for either of
#: them has to hold. Named so a case reads the two lines out of the fixture by what
#: the findings are about rather than by counting blank lines.
TWIN_CLAIM = "budget == 3"

#: One record where the schema holds two, for two functions that changed the same
#: way, and it records no line. The condition is the same text for both findings,
#: so the entry does not say which of the two its author read, and the run refuses
#: it rather than waiving one of them on a guess.
ONE_OF_TWO_WAIVERS = """{
  "_why": "The budget the first function names is a decision, not an accident.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the first function says so"
        }
      ]
    }
  }
}
"""

#: Both of them recorded, each naming the line of the finding it is about. Under
#: one leaf to a path there was nowhere to put the second entry, so a file raising
#: two findings under one rule could not be recorded at all however carefully a
#: person read it. The lines are what make two entries for the same claim mean two
#: things rather than one thing written twice.
TWIN_WAIVERS = """{
  "_why": "The budget both functions name is a decision, not an accident.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the first function says so",
          "line": LINE_FIRST
        },
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the second function says so",
          "line": LINE_SECOND
        }
      ]
    }
  }
}
"""

#: One record naming the second of the two identical findings by its line. Reading
#: the candidates in scan order instead of by line refused this entry every run and
#: said it named an assertion nobody recorded, while leaving both findings standing.
SECOND_LINE_WAIVERS = """{
  "_why": "The budget the second function names is a decision, not an accident.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the second function says so",
          "line": LINE
        }
      ]
    }
  }
}
"""

#: Both findings recorded again, with the second one written first. The order the
#: entries are in cannot matter, because each names its line, so this has to reach
#: the same two findings as the list above it.
REVERSED_LINES_WAIVERS = """{
  "_why": "Both budgets are decisions, and the second one is written first.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the second function says so",
          "line": LINE_SECOND
        },
        {
          "condition": "budget == 3",
          "reason": "the budget is now three, and the first function says so",
          "line": LINE_FIRST
        }
      ]
    }
  }
}
"""

#: A base whose assertion compares two values rather than claiming one. The claim a
#: finding carries here is the two values joined, so it is the one shape whose claim
#: is not a single normalised expression, and it is why the reader normalises both
#: sides of the comparison.
COMPARE_BASE = CLEAN_BASE.replace(
    '            assert!(fake.hide(), "the first hide must succeed");',
    '            assert_eq!(budget, 2, "a budget of two");',
).replace(
    "            let mut fake = Fake { budget: 1 };\n",
    "            let budget = 2;\n            let mut fake = Fake { budget };\n",
)

COMPARE_WORK = COMPARE_BASE.replace("budget, 2,", "budget, 3,")

#: The record for that comparison, with the condition written the way a person reads
#: it off the line: a space on each side of the `==`.
COMPARING_WAIVERS = """{
  "_why": "The budget the fake is given is a decision, not an accident.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "CONDITION",
          "reason": "the budget is three now, and the assertion says so",
          "line": LINE
        }
      ]
    }
  }
}
"""

#: A working tree that adds a comparison to a function that held no comparison at
#: the base, which is what makes it a finding about an added assertion rather than a
#: changed one. The claim such a finding carries is the two halves joined by `==`,
#: which is the spelling nobody would write by reading the line: the file says
#: `budget, 3`.
ADDED_COMPARING_WORK = CLEAN_BASE.replace(
    "            let mut fake = Fake { budget: 1 };\n",
    "            let budget = 3;\n"
    "            let mut fake = Fake { budget: 1 };\n"
    '            assert_eq!(budget, 3, "a budget of three");\n',
)

ADDED_WAIVERS = """{
  "_why": "The new comparison names the budget, which is a decision, not an accident.",
  "_waivers": {
    "R4-added": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "CONDITION",
          "reason": "the function now says what a budget of three looks like",
          "line": LINE
        }
      ]
    }
  }
}
"""

#: One line carrying two findings: the condition moved, and the message the change
#: added names nothing. Rule 1 judges the condition and rule 3 judges the message,
#: both on the same line and both about the same assertion, so the two findings
#: carry the same claim and differ only in which rule reported them. A record names
#: one rule, so this is the line where failing to name the rule would let a record
#: silence twice.
MIXED_WORK = BUDGET_BASE.replace(
    'assert!(budget == 2, "a budget of two: {budget:?}");',
    'assert!(budget == 3, "a budget of two: {:?}", budget);',
)

#: A base whose one function holds two assertions of a kind, with the working tree
#: holding one. The survivor matches the base assertion it replaced and its message
#: is untouched, so a removal is the only finding and the record written for it has
#: nothing else to agree with.
ONE_LEFT = CLEAN_BASE.replace(
    '            assert!(!fake.hide(), "a spent budget must not hide");\n', ""
)

#: A record for that removal. There is no line in it because a removal carries none:
#: nothing in the file says where a missing assertion was, and the claim is the
#: change itself rather than a line a person copied out.
REMOVAL_WAIVERS = """{
  "_why": "One of the two assertions went on purpose.",
  "_waivers": {
    "R1-removed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "CONDITION",
          "reason": "the spent-budget assertion moved to another file, where it belongs"
        }
      ]
    }
  }
}
"""


def _removal_claim(base_text: str, work_text: str) -> str:
    """What a removal finding is recognised by, read out of the two files.

    A removal has no line, so the claim is the change itself: the function, the
    kind, and the count on each side of it. Both counts are read from the fixtures
    rather than written here, because a count written next to the fixture it
    describes is a count that can be left behind when the fixture moves.
    """
    after = assertions_in(work_text)
    key = next(name for name, found in after.items() if found)
    return f"{key} {after[key][0].macro} {len(assertions_in(base_text)[key])}->{len(after[key])}"


def _write_budget_entry(root: Path, line: int) -> None:
    """The record for the budget finding, carrying a chosen line.

    The line is a parameter rather than read here because two cases disagree about
    it on purpose: one records the line the finding reports, and one records the
    line above it to see whether the run notices an entry that has slid.
    """
    _write_waivers(root, WAIVED_BUDGET.replace('"line": 0', f'"line": {line}'))


def _budget_line(root: Path) -> int:
    """The line the budget finding reports, read out of the working tree."""
    return _expected_lines(root, {"R1-changed": "budget == 3"})["R1-changed"]


def _absent_list_case(base: Path) -> Path:
    """A case whose recorded list has been deleted from the repository."""
    root = _fixture(base, "waiver-absent", BUDGET_WORK, base_text=BUDGET_BASE)
    (root / WAIVERS_PATH).unlink()
    return root


def _malformed_list_case(base: Path) -> Path:
    """A case whose recorded list is not the shape the reader expects."""
    root = _fixture(base, "waiver-malformed", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_waivers(root, LIST_AS_WAIVERS)
    return root


def _reasonless_case(base: Path) -> Path:
    """A case whose recorded entry says nothing about why it is recorded."""
    root = _fixture(base, "waiver-no-reason", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_waivers(root, NO_REASON_WAIVERS)
    return root


def _numbered_reason_case(base: Path) -> Path:
    """A case whose recorded entry gives a number where the reason should be.

    Every other list a person writes by hand has its mistake in a value rather than in
    a type, so this is the one shape where a refusal has to happen before the value is
    read as text. A run that gets as far as stripping the reason ends in a traceback
    rather than a refusal, and the self-test is what turns that into a red case.
    """
    root = _fixture(base, "waiver-numbered-reason", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_waivers(root, NUMBERED_REASON_WAIVERS.replace("LINE", str(_budget_line(root))))
    return root


def _unknown_rule_case(base: Path) -> Path:
    """A case whose recorded entry names a rule id the check never reports."""
    root = _fixture(base, "waiver-unknown-rule", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_waivers(root, UNKNOWN_RULE_WAIVERS.replace("LINE", str(_budget_line(root))))
    return root


def _wrong_rule_case(base: Path) -> Path:
    """A case whose recorded entry is filed under the rule beside the right one.

    The two ids begin with the same two characters, so this is the record a check
    comparing anything less than the whole id would honour, and the finding it names is
    one of another kind. It has to stay reported, because a finding somebody judged by
    accident is a finding nobody read at all.
    """
    root = _fixture(base, "waiver-wrong-rule", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_waivers(root, WRONG_RULE_WAIVERS.replace("LINE", str(_budget_line(root))))
    return root


def _nothing_compared_case(base: Path) -> Path:
    """A case whose run compared no file at all, and the list still holds a record.

    A clean tree compares nothing, so this is the shape every unrelated commit has: the
    record is named, waives nothing and fails nothing. Naming a path the run did not
    compare as though there were some to compare would give the reader a path to open
    and find nothing in.
    """
    root = _fixture(base, "waiver-nothing-compared", None, base_text=CLEAN_BASE)
    _write_waivers(root, UNEXERCISED_WAIVERS)
    return root


def _unreadable_source_case(base: Path) -> Path:
    """A case whose one source file is there and cannot be read.

    The path is replaced by a directory, so the run has to open the file and
    cannot. A file that is simply gone is not this case: the scan treats a file
    the base held and the tree deleted as making no claim about an assertion
    that used to exist, so a deletion is not a failure, and a case built on one
    can never see the code it was written for. What fails the run is a path it
    must read and cannot, and the summary under this code has to be the one
    about paths and not the one about a record that is gone.
    """
    root = _fixture(base, "waiver-unreadable-source")
    target = root / FIXTURE_PATH
    target.unlink()
    target.mkdir()
    return root


def _stale_entry_case(base: Path) -> Path:
    """A case whose recorded entry matches no finding this run reports."""
    root = _fixture(base, "waiver-stale", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_waivers(root, STALE_WAIVERS)
    return root


def _spent_entry_case(base: Path) -> Path:
    """A case whose recorded entry still describes an assertion the file holds.

    The change is a comment: the file differs from the base, so the run compares
    it, and no assertion moved, so no finding exists for the entry to match. The
    entry then has to be told apart from one whose assertion is gone, and this is
    the landed half of that choice. The entry records the line the claim actually
    sits on, which is the spent rule's own condition.
    """
    root = _fixture(
        base,
        "waiver-spent",
        BUDGET_BASE + "\n// A change that touches the file and no assertion.\n",
        base_text=BUDGET_BASE,
    )
    line = _expected_line(root, "budget == 2")
    _write_waivers(root, SPENT_WAIVERS.replace("LINE", str(line)))
    return root


def _slid_landed_case(base: Path) -> Path:
    """A case whose recorded claim sits in the file, but not on the recorded line.

    The work changes a different assertion, so a finding exists to fail the run
    and the recorded claim itself is untouched. The entry records the line
    beside the claim instead of its own, and that stays a failure: the line is
    what tells the record which assertion its author read, and the check already
    refuses a line that no matching finding sits on.
    """
    root = _fixture(base, "waiver-slid-landed", SLID_WORK, base_text=BUDGET_BASE)
    line = _expected_line(root, "budget == 2") + 1
    _write_waivers(root, SLID_LANDED_WAIVERS.replace("LINE", str(line)))
    return root


def _two_findings_case(base: Path) -> Path:
    """A case whose one record could have stood for either of two findings.

    The two findings carry the same rule, the same file and the same condition, and
    the entry records no line, so it does not say which of them its author read. The
    run waives neither and says why, which is the safe direction: a record taken on
    a guess would leave the other finding silently gone.
    """
    root = _fixture(base, "waiver-two", TWIN_WORK, base_text=TWIN_BASE)
    _write_waivers(root, ONE_OF_TWO_WAIVERS)
    return root


def _two_recorded_case(base: Path) -> Path:
    """A case whose path records both of the findings one file raised.

    The same two functions as the case above, recorded in full with each entry
    naming its line. Under one leaf to a path there was nowhere to put the second
    entry, so a file with two findings under one rule could not be recorded at all.
    """
    root = _fixture(base, "waiver-two-recorded", TWIN_WORK, base_text=TWIN_BASE)
    _write_waivers(root, TWIN_WAIVERS.replace(
        "LINE_FIRST", str(_expected_line(root, TWIN_CLAIM))
    ).replace("LINE_SECOND", str(_expected_line(root, TWIN_CLAIM, 2))))
    return root


def _second_line_case(base: Path) -> Path:
    """A case whose record names the second of two identical findings by its line.

    One entry, one line, and the finding on that line is the one waived. The other
    finding stays a finding, because nobody recorded it. The line is what decided
    this: an entry is matched against the candidates sitting on the line it
    recorded, and the finding it waived is the second of the two, not the first.
    """
    root = _fixture(base, "waiver-second-line", TWIN_WORK, base_text=TWIN_BASE)
    _write_waivers(root, SECOND_LINE_WAIVERS.replace(
        "LINE", str(_expected_line(root, TWIN_CLAIM, 2))
    ))
    return root


def _reversed_lines_case(base: Path) -> Path:
    """A case whose two records are written in the opposite order to their lines.

    Both findings are named by their own line, so the order they were written in
    cannot matter. A run that took the first candidate it found would waive the
    second line's finding first and leave the other standing, which is the pair of
    waivers with the same count and the wrong lines.
    """
    root = _fixture(base, "waiver-lines-reversed", TWIN_WORK, base_text=TWIN_BASE)
    _write_waivers(root, REVERSED_LINES_WAIVERS.replace(
        "LINE_FIRST", str(_expected_line(root, TWIN_CLAIM))
    ).replace("LINE_SECOND", str(_expected_line(root, TWIN_CLAIM, 2))))
    return root


def _comparing_macro_case(base: Path) -> Path:
    """A case recording a finding about `assert_eq!`.

    The condition a person writes is the assertion's own text, spaces and all. The
    claim the scanner builds for a comparing macro is two normalised halves joined
    with a spaced `==`, and that joined text was never normalised as a whole, so it
    held a space the reader's `normalise` dropped and no entry for one of these
    could ever match.
    """
    root = _fixture(base, "waiver-comparing", COMPARE_WORK, base_text=COMPARE_BASE)
    _write_waivers(root, COMPARING_WAIVERS.replace("CONDITION", "budget == 3").replace(
        "LINE", str(_expected_lines(root, {"R1-changed": "budget, 3,"})["R1-changed"])
    ))
    return root


def _wrong_line_case(base: Path) -> Path:
    """A case whose recorded line is the one above the finding."""
    root = _fixture(base, "waiver-wrong-line", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_budget_entry(root, _budget_line(root) - 1)
    return root


def _other_rule_case(base: Path) -> Path:
    """A case recording one of the two findings its single line carries."""
    root = _fixture(base, "waiver-other-rule", MIXED_WORK, base_text=BUDGET_BASE)
    _write_budget_entry(root, _budget_line(root))
    return root


def _recorded_removal_case(base: Path) -> Path:
    """A case whose recorded entry is for a removal, which carries no line."""
    root = _fixture(base, "waiver-removal", ONE_LEFT)
    _write_waivers(root, REMOVAL_WAIVERS.replace(
        "CONDITION", _removal_claim(CLEAN_BASE, ONE_LEFT)
    ))
    return root


def _added_comparing_case(base: Path) -> Path:
    """A case recording a finding about a comparison the change added.

    The entry's condition is the claim the finding carries, which is the text a
    reader has to be able to copy out of the printed line. The claim of a comparing
    macro is the two halves joined by `==`, so it is not the `budget, 3` the file
    holds, and a printed line that named only the macro and the function left a
    reader to guess the spelling the check matches on.
    """
    root = _fixture(base, "waiver-added-comparing", ADDED_COMPARING_WORK)
    _write_waivers(root, ADDED_WAIVERS.replace(
        "CONDITION", _printed_claim(root, "R4-added")
    ).replace("LINE", str(_expected_line(root, "assert_eq!(budget, 3,"))))
    return root


def _printed_removal_case(base: Path) -> Path:
    """A case recording a removal with the claim the printed line carries.

    A removal carries no line, so the claim is the whole change and there is nothing
    in the file to copy it from. The only place a reader can get it is the line the
    finding printed, which is why the claim goes in the message as well as in the
    comparison.
    """
    root = _fixture(base, "waiver-removal-printed", ONE_LEFT)
    _write_waivers(root, REMOVAL_WAIVERS.replace(
        "CONDITION", _printed_claim(root, "R1-removed")
    ))
    return root


def _verdict_case(base: Path) -> Path:
    """A case whose one finding is recorded, so the run is green with a judgement in it."""
    root = _fixture(base, "verdict-waived", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_budget_entry(root, _budget_line(root))
    return root


#: A list that writes its own top key twice, which is what a hand edit leaves behind
#: when a person adds a block by copying the one above it. The parser keeps the last of
#: the two, so the record the first one held would go missing with nothing to say that a
#: record had ever been written.
REPEATED_TOP_LEVEL = """{
  "_why": "One key written twice, by hand, while adding a block above it.",
  "_waivers": {
    "R1-changed": {}
  },
  "_waivers": {
    "R1-changed": {}
  }
}
"""

#: A list that writes the same path twice under one rule, which is the same mistake one
#: layer down. Two records for one path are two entries in a list and are read as two;
#: the same key twice is one object claiming two things about one path, and only the
#: second is ever read.
REPEATED_PATH = """{
  "_why": "One path written twice, by hand, while adding a second record to it.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is three now, and the first entry says so"
        }
      ],
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 4",
          "reason": "the budget is four now, and the second entry says so"
        }
      ]
    }
  }
}
"""


def _repeated_key_case(base: Path, label: str, text: str) -> Path:
    """A case whose recorded list writes one key twice.

    The change is the budget one, so the run has a finding to report and the refusal is
    about the list rather than about a tree that happened to be clean. Two records for
    one path are not a repeated key and stay legal, which is why each of these puts the
    repeat in the object above the list rather than beside an entry.
    """
    root = _fixture(base, label, BUDGET_WORK, base_text=BUDGET_BASE)
    _write_waivers(root, text)
    return root


def _linked_list_case(base: Path) -> Path:
    """A case whose recorded list is a link to a perfectly good list.

    Nothing about the change is wrong, so a run that followed the link would waive the
    finding and report a clean tree. The link is committed as well, so the index holds
    the target's name as text where the list should be, and the run reading the index
    refuses it. One path, two answers, and only one of them ever seen by whoever made
    the link.
    """
    root = _fixture(base, "waiver-link", BUDGET_WORK, base_text=BUDGET_BASE)
    _write_budget_entry(root, _budget_line(root))
    held = root / "scripts/held_list.json"
    (root / WAIVERS_PATH).rename(held)
    (root / WAIVERS_PATH).symlink_to("held_list.json")
    _git_in(root, "add", "-A")
    return root


def _list_directory_case(base: Path) -> Path:
    """A case whose recorded list is a directory, with a good list in the index.

    The same disagreement seen from the other side: the record is committed, so a run
    reading the index would honour it and call the run clean, while the path in the
    working tree is not a file at all. Both runs have to refuse it, or the same tree
    keeps answering differently depending on who is asking.
    """
    root = _fixture(
        base, "waiver-link-staged", BUDGET_WORK, base_text=BUDGET_BASE, staged=True
    )
    (root / FIXTURE_PATH).write_text(BUDGET_WORK, encoding="utf-8")
    _write_budget_entry(root, _budget_line(root))
    _git_in(root, "add", WAIVERS_PATH)
    (root / WAIVERS_PATH).unlink()
    (root / WAIVERS_PATH).mkdir()
    return root


#: One of each thing a run can say about the recorded list, in one tree: a finding a
#: person recorded, a record that no longer describes the tree, and a record for a file
#: the run never opened. The twin base gives two identical findings so that one of them
#: can be recorded while the other stays a finding, because the printed contract has a
#: line for each kind and a run holding only one of them cannot pin an order.
ONE_OF_EACH = """{
  "_why": "One recorded finding, one record that no longer describes the tree, and one for a file this run never opened.",
  "_waivers": {
    "R1-changed": {
      "crates/demo/src/fake.rs": [
        {
          "condition": "budget == 3",
          "reason": "the budget is three now, and the second function says so",
          "line": LINE_SECOND
        },
        {
          "condition": "budget == 9",
          "reason": "written for a version of the assertion that is gone"
        }
      ],
      "crates/demo/src/untouched.rs": [
        {
          "condition": "budget() == 7",
          "reason": "the seventh call is the one that must fail",
          "line": 3
        }
      ]
    }
  }
}
"""


def _printed_contract_case(base: Path) -> Path:
    """A case holding one finding standing, one waived, one stale and one unexercised.

    The printed output is all an operator ever sees and a commit hook keeps only the
    lines beginning with its own prefix, so a line without it is swallowed and the hook
    stops matching what a build prints. A deletion of every waived line and both counts
    once left every case green, so the shape of the output is pinned here rather than
    left to be read.
    """
    root = _fixture(base, "printed", TWIN_WORK, base_text=TWIN_BASE)
    _write_waivers(root, ONE_OF_EACH.replace(
        "LINE_SECOND", str(_expected_line(root, TWIN_CLAIM, 2))
    ))
    return root


#: One record for a file the run will never open, written out as a person would write
#: it rather than through a serialiser, because the list is the thing a person copies
#: and a serialiser's spacing is not what they type. A function rather than a template
#: because the braces in the text are the list's own and a placeholder would swallow
#: them.
def spare_record(number: int) -> str:
    """The `number`th record, for a file no run over this fixture will compare."""
    return (
        f'      "crates/demo/src/spare{number}.rs": [\n'
        "        {\n"
        f'          "condition": "budget == {number}",\n'
        f'          "reason": "the call numbered {number} is the one that must fail",\n'
        '          "line": 3\n'
        "        }\n"
        "      ]"
    )


def many_unexercised(count: int) -> str:
    """A list holding `count` records for files the run will never open.

    Two more than the cap shows, so a line that named every record and a line that
    stopped at the cap read differently, and the remainder is something a reader can be
    told about rather than something they have to notice is missing.
    """
    entries = ",\n".join(spare_record(number) for number in range(1, count + 1))
    return (
        "{\n"
        '  "_why": "Judgements recorded while the tree stood still.",\n'
        '  "_waivers": {\n'
        '    "R1-changed": {\n'
        f"{entries}\n"
        "    }\n"
        "  }\n"
        "}\n"
    )


def _capped_list_case(base: Path) -> Path:
    """A case whose list holds more records for unopened files than the line shows."""
    root = _fixture(base, "waiver-cap", MESSAGED)
    _write_waivers(root, many_unexercised(NOT_EXERCISED_SHOWN + 2))
    return root


def _staged_list_case(base: Path, label: str, added: bool) -> Path:
    """A case whose recorded entry is in the index, or on the disk only.

    The source file is put back on the disk after staging, so the two runs differ
    only in where the entry is. That is what makes the second one the negative of
    the first: the staged run reads the index for the list, so a disk copy holding
    nothing is exactly what it must see for the entry to be honoured, and a disk
    copy holding an entry it must ignore is what proves it.
    """
    root = _fixture(base, label, BUDGET_WORK, base_text=BUDGET_BASE, staged=True)
    (root / FIXTURE_PATH).write_text(BUDGET_WORK, encoding="utf-8")
    _write_budget_entry(root, _budget_line(root))
    if added:
        _git_in(root, "add", WAIVERS_PATH)
        # What a person reading the working tree now sees is a list holding
        # nothing, while the commit about to be made carries the entry.
        _write_waivers(root, NO_WAIVERS)
    return root


def _unstaged_list_case(base: Path) -> Path:
    """A case whose recorded list is on the disk and gone from the index.

    `git rm --cached` is how a record is dropped without the working tree changing,
    so the list is still there for a run that reads the disk and absent for a staged
    run. It is the one way the two can disagree, and the staged run has to say so
    rather than read an absence as a list holding nothing.
    """
    root = _fixture(base, "waiver-unstaged", BUDGET_WORK, base_text=BUDGET_BASE, staged=True)
    _git_in(root, "rm", "--cached", "--quiet", WAIVERS_PATH)
    return root


def _printed_claim(root: Path, rule: str) -> str:
    """The claim a record has to hold, read out of the finding the run reported.

    Read from the finding rather than rebuilt from the two fixtures, because the
    point of the cases using it is that a reader copies the claim out of the line a
    run printed. A case that rebuilt the text would hold whatever the check happens
    to compare against, and would pass against a check whose printed line named
    nothing anybody could copy.
    """
    surface = scan(root, "HEAD")
    return next(finding.claim for finding in surface.findings if finding.rule == rule)


def _expected_line(root: Path, needle: str, nth: int = 1) -> int:
    """The `nth` line of the fixture's working tree holding `needle`, counting from one.

    The count is a parameter because two identical assertions are the shape every
    line-named record is about, and a hand-counted line for either of them would rot
    the first time the fixture moved.
    """
    source = (root / FIXTURE_PATH).read_text(encoding="utf-8").splitlines()
    return [
        number for number, raw in enumerate(source, start=1) if needle in raw
    ][nth - 1]


def _held_removed_test(surface: Surface, root: Path) -> str | None:
    """A removed test function is reported for sign-off, and fails nothing.

    No rule can follow a rename: the function's assertions are unmatched and
    skipped, so the exit code stays green however many tests a rewrite takes
    out. The printed line is the whole deliverable, so it is pinned here.
    """
    if surface.findings or surface.waived:
        return (
            f"left {len(surface.findings)} finding(s) and waived "
            f"{len(surface.waived)}, wanted a green run"
        )
    if len(surface.removed_tests) != 1:
        return f"{len(surface.removed_tests)} test function(s) reported, wanted one"
    said = [line for line in _printed_lines(root) if "REMOVED TEST" in line]
    if len(said) != 1:
        return f"{len(said)} lines about the removed function, wanted one"
    if "fake.tests::spends_one_call" not in said[0]:
        return f"the line does not name the function: {said[0]!r}"
    return None


def _held_matched(surface: Surface, root: Path) -> str | None:
    """A recorded finding is judged as recorded rather than dropped on the floor.

    The two lists are read because an exit code of 0 is the same answer for a
    finding that was waived and for one that was never reported at all.
    """
    if len(surface.findings) != 0 or len(surface.waived) != 1 or surface.stale_waivers:
        return (
            f"left {len(surface.findings)} finding(s), {len(surface.waived)} waived "
            f"and {len(surface.stale_waivers)} stale"
        )
    return None


def _held_unexercised(surface: Surface, root: Path) -> str | None:
    """An entry this run never reached is reported as that, and not called stale.

    The run opened no file, so it has nothing to say about what was recorded for
    one, and a record it cannot judge is not a record that has stopped describing
    the tree. The line also has to be about the recorded path rather than about the
    run: a path spelled wrongly is a mistake in the entry, and saying the run did
    not compare the file tells the reader to go and look at the code instead. The
    paths the run did compare go on the same line, because a reader who cannot see
    that the run opened anything has no way to tell a wrong path from a path the
    change never touched.
    """
    if len(surface.unexercised_waivers) != 1 or surface.stale_waivers or surface.waived:
        return (
            f"left {len(surface.unexercised_waivers)} not exercised, "
            f"{len(surface.stale_waivers)} stale and {len(surface.waived)} waived"
        )
    said = [line for line in _printed_lines(root) if "NOT EXERCISED" in line]
    if not said:
        return "the run printed no line about the entry it could not judge"
    if "did not compare that file" in said[0]:
        return f"the line still claims the run did not look: {said[0]!r}"
    if "not one of the paths this run compared" not in said[0]:
        return f"the line does not say what is wrong with the recorded path: {said[0]!r}"
    # The path is written as a literal rather than read off the surface, because a line
    # naming the paths from the same list the case then checks is a line nothing pins.
    if FIXTURE_PATH not in said[0].split("; this run compared")[-1]:
        return f"the line does not name a path the run did compare: {said[0]!r}"
    return None


def _held_nothing_compared(surface: Surface, root: Path) -> str | None:
    """A run that opened nothing says so on the line, and names no path.

    Naming a path the run did not compare as though there were some is the same defect
    as naming none at all: the reader goes to look for a path that does not exist and
    learns nothing about the record they actually have to fix.
    """
    if len(surface.unexercised_waivers) != 1 or surface.stale_waivers:
        return (
            f"left {len(surface.unexercised_waivers)} not exercised and "
            f"{len(surface.stale_waivers)} stale"
        )
    if surface.scanned:
        return f"compared {len(surface.scanned)} file(s), wanted none"
    said = [line for line in _printed_lines(root) if "NOT EXERCISED" in line]
    if len(said) != 1:
        return f"{len(said)} lines about the records not judged, wanted one"
    if "compared no file at all" not in said[0]:
        return f"the line does not say the run opened nothing: {said[0]!r}"
    return None


def _held_gone_source(surface: Surface, root: Path) -> str | None:
    """Only a source file is unreadable, so the summary printed is the one about paths.

    The summary is pinned as a literal because this code is shared by a missing record
    and a missing file, and the two sentences say different causes. A sentence about the
    record being gone, printed here, would send the reader to open a list that is there.
    """
    if len(surface.unreadable) != 1:
        return f"{len(surface.unreadable)} unreadable, wanted only the source file"
    if WAIVERS_PATH in surface.unreadable[0]:
        return f"the refusal is about the list: {surface.unreadable[0]}"
    if FIXTURE_PATH not in surface.unreadable[0]:
        return f"the refusal does not name the source file: {surface.unreadable[0]}"
    wanted = (
        f"{NAME}: 1 of the things this check had to read could not be read, and the rest "
        "of this run does not cover them"
    )
    said = [line for line in _printed_lines(root) if "could not be read" in line]
    if said != [wanted]:
        return f"printed {said!r}, wanted the summary about paths and nothing else"
    return None


def _held_numbered_reason(surface: Surface, root: Path) -> str | None:
    """A reason that is not a string is refused by name, and nothing crashes.

    The case running at all is half of this: a reader that tested the type and then
    treated the value as text would raise rather than return, and the self-test would
    end in a traceback instead of reporting a red case. So the other half is the field
    being named, because a refusal that says only that the list is wrong leaves the
    person who wrote the entry to guess which of the two text fields it was.
    """
    problem = _held_names_the_list(surface, root)
    if problem is not None:
        return problem
    if len(surface.unreadable) != 1:
        return f"{len(surface.unreadable)} unreadable, wanted one"
    if "reason" not in surface.unreadable[0]:
        return f"the refusal does not name the field: {surface.unreadable[0]}"
    if surface.waived:
        return f"waived {len(surface.waived)} of a list that was refused"
    return None


def _held_unknown_rule(surface: Surface, root: Path) -> str | None:
    """A rule id the check never reports is refused, and the refusal names it.

    The id is written as a literal rather than read from the set of rules, because a
    reader that refuses whatever the set says has said nothing about the rule itself.
    A refusal that named the set but not the id would also leave the reader to guess
    which of the six they typed wrong.
    """
    problem = _held_names_the_list(surface, root)
    if problem is not None:
        return problem
    if len(surface.unreadable) != 1:
        return f"{len(surface.unreadable)} unreadable, wanted one"
    if "'R1-changedd'" not in surface.unreadable[0]:
        return f"the refusal does not name the rule it refused: {surface.unreadable[0]}"
    if surface.waived:
        return f"waived {len(surface.waived)} of a list that was refused"
    return None


def _held_wrong_rule(surface: Surface, root: Path) -> str | None:
    """A record under one rule silences nothing of another that shares its first part."""
    if len(surface.findings) != 1 or surface.findings[0].rule != "R1-changed":
        return (
            f"left {[finding.rule for finding in surface.findings]}, wanted the changed "
            "claim standing"
        )
    if surface.waived:
        return f"waived {len(surface.waived)}, wanted none of them"
    if len(surface.stale_waivers) != 1:
        return f"{len(surface.stale_waivers)} entries called out, wanted the one filed wrong"
    return None


def _held_repeated_top(surface: Surface, root: Path) -> str | None:
    """A key written twice is refused, and the refusal names the key.

    The parser keeps the last of the two and says nothing, so without this the record
    the first one held would go missing and the finding it was written for would go
    stale with nothing to say that a record had ever been there.
    """
    problem = _held_names_the_list(surface, root)
    if problem is not None:
        return problem
    # The key is written as a literal rather than read from the constant, because a
    # check built from the same constant as the parser passes when the parser changes.
    if "'_waivers'" not in surface.unreadable[0]:
        return f"the refusal does not name the repeated key: {surface.unreadable[0]}"
    return None


def _held_repeated_path(surface: Surface, root: Path) -> str | None:
    """A path written twice is refused, and the refusal says which layer it is in.

    The layer is the part a reader cannot work out from the message alone: a path
    written twice and a rule written twice produce the same kind of trouble, and the
    one that fixes it is different.
    """
    problem = _held_names_the_list(surface, root)
    if problem is not None:
        return problem
    if f"'{FIXTURE_PATH}'" not in surface.unreadable[0]:
        return f"the refusal does not name the repeated path: {surface.unreadable[0]}"
    if "the entries for R1-changed" not in surface.unreadable[0]:
        return f"the refusal does not say which layer: {surface.unreadable[0]}"
    return None


def _held_linked_list(surface: Surface, root: Path) -> str | None:
    """A list that is not a regular file is refused in both modes, findings and all.

    The disk follows a link and the index holds the target's name as text, so the same
    tree answers differently depending on who is asking, and the answer the person who
    made the link sees is the one nobody else ever sees. The findings still print,
    because a refusal that swallowed them would leave a reader with nothing to act on.
    """
    problem = _held_names_the_list(surface, root)
    if problem is not None:
        return problem
    if "not a regular file" not in surface.unreadable[0]:
        return f"the refusal does not say the list is not a file: {surface.unreadable[0]}"
    if not surface.findings:
        return "the finding went with the list"
    if surface.waived:
        return f"waived {len(surface.waived)} of a list that was never read"
    return None


def _held_printed_contract(surface: Surface, root: Path) -> str | None:
    """The printed contract: one line per kind, each beginning where the hook keeps it.

    A commit hook keeps only the lines beginning with its own prefix, so a line without
    it is swallowed and the hook's output stops matching what a build prints. The counts
    are pinned as literals rather than read back off the surface, because a count
    printed from the same number the case then checks is a count nothing is pinned
    against.
    """
    printed = _printed_lines(root)
    swallowed = [line for line in printed if not line.startswith(f"{NAME}:")]
    if swallowed:
        return f"a line the commit hook would swallow: {swallowed[0]!r}"
    kinds = []
    for line in printed:
        if line.startswith(f"{NAME}: FINDING "):
            kinds.append("FINDING")
        elif line.startswith(f"{NAME}: WAIVED "):
            kinds.append("WAIVED")
        elif line.startswith(f"{NAME}: STALE WAIVER "):
            kinds.append("STALE")
        elif line.startswith(f"{NAME}: WAIVER NOT EXERCISED "):
            kinds.append("NOT EXERCISED")
        else:
            kinds.append("count")
    wanted = ["FINDING", "WAIVED", "STALE", "NOT EXERCISED", "count"]
    if kinds != wanted:
        return f"printed {kinds}, wanted {wanted}"
    counts = f"{NAME}: 1 finding(s), 1 waived, 1 stale waiver(s)"
    if printed[-1] != counts:
        return f"the count line read {printed[-1]!r}, wanted {counts!r}"
    return None


def _held_capped(surface: Surface, root: Path) -> str | None:
    """One line, capped, with the number it did not show printed beside it.

    The list grows one entry per judgement and a line per entry is printed on every
    unrelated run, which is what the single line replaced. A cap that did not say what
    it left out would be a line a reader had to trust, so the remainder is counted.
    """
    over = NOT_EXERCISED_SHOWN + 2
    if len(surface.unexercised_waivers) != over:
        return f"{len(surface.unexercised_waivers)} not judged, wanted {over}"
    if surface.stale_waivers or surface.findings:
        return (
            f"left {len(surface.findings)} finding(s) and "
            f"{len(surface.stale_waivers)} called out, wanted neither"
        )
    said = [line for line in _printed_lines(root) if "NOT EXERCISED" in line]
    if len(said) != 1:
        return f"{len(said)} lines about the records not judged, wanted one"
    if f"{over} recorded entries" not in said[0]:
        return f"the line does not carry the count: {said[0]!r}"
    if f"and {over - NOT_EXERCISED_SHOWN} more not shown" not in said[0]:
        return f"the line does not count what it left out: {said[0]!r}"
    named = [record for record in surface.unexercised_waivers if record in said[0]]
    if len(named) != NOT_EXERCISED_SHOWN:
        return f"the line named {len(named)} records, wanted {NOT_EXERCISED_SHOWN}"
    return None


def _held_absent(surface: Surface, root: Path) -> str | None:
    """A list that is not there fails the run, and takes no finding with it.

    An absent list and a list holding nothing are different states, and only one of
    them means nobody recorded anything. Reporting the finding as well is what says
    the run got as far as judging the tree: a refusal that also swallowed the
    findings would leave a reader with nothing to act on. The summary under that
    refusal names the record as gone rather than the paths, because what is missing
    is the record of every judgement and not a file, and it counts the files the run
    did compare so a reader can see the tree was still judged. The count is pinned as
    a literal because a count read back off the surface is a count nothing holds.
    """
    if not any(WAIVERS_PATH in said for said in surface.unreadable):
        return f"the run did not say it could not read {WAIVERS_PATH}"
    if not surface.findings:
        return "the finding went with the list"
    said = [line for line in _printed_lines(root) if "could not be read" in line]
    if len(said) != 1:
        return f"{len(said)} summaries of what could not be read, wanted one"
    if "the recorded list could not be read" not in said[0]:
        return f"the summary does not name the record as gone: {said[0]!r}"
    if "compared 1 file(s) without it" not in said[0]:
        return f"the summary does not count the files the run did compare: {said[0]!r}"
    if "path(s)" in said[0]:
        return f"the summary calls the missing record a path: {said[0]!r}"
    return None


def _held_names_the_list(surface: Surface, root: Path) -> str | None:
    """A refused list is named in the refusal.

    A reader who has to guess which file was wrong cannot open it, and a run that
    printed a reason without a path has failed in a way its caller cannot act on.
    """
    if not any(WAIVERS_PATH in said for said in surface.unreadable):
        return f"the refusal did not name {WAIVERS_PATH}"
    return None


def _held_refused(surface: Surface, root: Path) -> str | None:
    """An entry that does not describe a finding waives nothing and says so."""
    if len(surface.stale_waivers) != 1:
        return f"{len(surface.stale_waivers)} entries called out, wanted 1"
    if not surface.findings or surface.waived:
        return (
            f"left {len(surface.findings)} finding(s) and waived "
            f"{len(surface.waived)}"
        )
    return None


def _held_stale_text(surface: Surface, root: Path) -> str | None:
    """A refused entry is told what it recorded, not that the tree moved.

    Saying the entry stopped describing the tree sends the reader looking at the
    code, and the code is not what is wrong when the author spelled the claim
    differently. The line has to name the recorded text, because that is the thing
    the reader can fix.
    """
    problem = _held_refused(surface, root)
    if problem is not None:
        return problem
    said = [line for line in _printed_lines(root) if "STALE WAIVER" in line]
    if not said:
        return "the run printed no line about the refused entry"
    if "no longer describes the tree" in said[0]:
        return f"the refusal still blames the tree: {said[0]!r}"
    if "recorded claim" not in said[0]:
        return f"the refusal does not name the recorded claim: {said[0]!r}"
    return None


def _held_spent(surface: Surface, root: Path) -> str | None:
    """A landed entry is reported as spent, and the run stays green.

    The printed line matters as much as the list: a reader who never opens the
    source still has to see which entry can be deleted and why the run did not
    fail over it.
    """
    if surface.findings:
        return f"{len(surface.findings)} finding(s) left, wanted none"
    if surface.waived:
        return f"waived {len(surface.waived)}, wanted the entry spent rather than taken"
    if len(surface.spent_waivers) != 1:
        return (
            f"{len(surface.spent_waivers)} entries called out as spent, wanted the one "
            "filed for the change that landed"
        )
    if surface.stale_waivers:
        return f"{len(surface.stale_waivers)} entries called out as stale, wanted none"
    said = [line for line in _printed_lines(root) if "SPENT WAIVER" in line]
    if not said:
        return "the run printed no line about the spent entry"
    if "recorded claim" not in said[0]:
        return f"the spent line does not name the recorded claim: {said[0]!r}"
    return None


def _held_slid_landed(surface: Surface, root: Path) -> str | None:
    """A claim the file holds on another line fails, and the message names both.

    Spent here would say the entry can be deleted while hiding that its line is
    wrong, and stale-without-detail would send the reader looking for an
    assertion that is still there. The line the claim sits on and the line the
    record holds are the two facts that tell them apart.
    """
    if surface.spent_waivers:
        return (
            f"{len(surface.spent_waivers)} entries called out as spent, wanted the "
            "slid one to fail"
        )
    if len(surface.stale_waivers) != 1:
        return f"{len(surface.stale_waivers)} entries called out, wanted the slid one"
    said = [line for line in _printed_lines(root) if "STALE WAIVER" in line]
    if not said:
        return "the run printed no line about the slid entry"
    if "not the recorded line" not in said[0]:
        return f"the refusal does not name the recorded line: {said[0]!r}"
    if str(_expected_line(root, "budget == 2")) not in said[0]:
        return f"the refusal does not name the line the claim sits on: {said[0]!r}"
    return None


def _held_slid_text(surface: Surface, root: Path) -> str | None:
    """An entry that has slid is told the line it named and the line the claim is on.

    It used to be told it named an assertion nobody recorded, which is false: its
    author read an assertion and wrote the wrong line for it, and a sentence saying
    nobody recorded it sends the reader looking for a second judgement instead of at
    the number in front of them.
    """
    problem = _held_refused(surface, root)
    if problem is not None:
        return problem
    said = [line for line in _printed_lines(root) if "STALE WAIVER" in line]
    if not said:
        return "the run printed no line about the entry that slid"
    if "nobody recorded" in said[0]:
        return f"the refusal still says nobody recorded it: {said[0]!r}"
    if f"line {str(_budget_line(root) - 1)}" not in said[0]:
        return f"the refusal does not name the recorded line: {said[0]!r}"
    return None


def _held_second_line(surface: Surface, root: Path) -> str | None:
    """The recorded line, not the order, decides which of two identical findings went."""
    if len(surface.waived) != 1:
        return f"waived {len(surface.waived)}, wanted exactly the one the line named"
    if surface.waived[0].line != _expected_line(root, TWIN_CLAIM, 2):
        return f"waived the finding on line {surface.waived[0].line}, wanted the second"
    if len(surface.findings) != 1:
        return f"left {len(surface.findings)} finding(s), wanted the one nobody recorded"
    if surface.stale_waivers:
        return f"{len(surface.stale_waivers)} entries called out, wanted none"
    return None


def _held_both_lines(surface: Surface, root: Path) -> str | None:
    """Two line-named entries waive the two findings, whichever order they are in."""
    if len(surface.waived) != 2:
        return f"waived {len(surface.waived)}, wanted both"
    if surface.findings or surface.stale_waivers:
        return (
            f"left {len(surface.findings)} finding(s) and "
            f"{len(surface.stale_waivers)} entries called out, wanted neither"
        )
    wanted = sorted({_expected_line(root, TWIN_CLAIM), _expected_line(root, TWIN_CLAIM, 2)})
    got = sorted(finding.line for finding in surface.waived)
    if got != wanted:
        return f"waived lines {got}, wanted {wanted}"
    return None


def _held_printed_claim(surface: Surface, root: Path) -> str | None:
    """The claim a record must hold is written out in the line the run printed.

    An entry holding the claim is honoured, which is the first half. The claim
    appearing in the printed line is the second half, and it is the half that decides
    whether a person can write the entry at all: a finding that names only a macro
    and a function leaves them guessing at a spelling, and a guess that is wrong is
    reported as though they had recorded the wrong judgement.
    """
    if len(surface.waived) != 1:
        return f"waived {len(surface.waived)}, wanted exactly the recorded finding"
    finding = surface.waived[0]
    if f"`{finding.claim}`" not in str(finding):
        return f"the printed line does not carry the claim `{finding.claim}`"
    return None


def _held_verdict(surface: Surface, root: Path) -> str | None:
    """A green run with a recorded judgement says so inside the sentence.

    The old sentence claimed every assertion still asserts the same thing and then
    counted the recorded ones after it, which is false on exactly the run this case
    builds and is the line a reader who reads the end of a log actually sees. The
    clean half is here too, because the wording on a clean run is what a hook and its
    reader both know, and neither may change under this fix.
    """
    if len(surface.waived) != 1:
        return f"waived {len(surface.waived)}, wanted exactly the recorded one"
    printed = _printed_lines(root)
    verdict = printed[-1] if printed else ""
    if "every assertion still asserts the same thing" in verdict:
        return f"the verdict still claims it: {verdict!r}"
    if f"the {len(surface.waived)} recorded above" not in verdict:
        return f"the verdict does not name the recorded judgement: {verdict!r}"
    clean = scan(_fixture(root.parent, "verdict-clean", MESSAGED), "HEAD")
    wanted = (
        f"{NAME}: every assertion still asserts the same thing, and every message a "
        f"change added is well formed ({len(clean.scanned)} file(s) compared)"
    )
    control = _printed_lines(root.parent / "verdict-clean")
    if not control or control[-1] != wanted:
        return f"a clean run printed {control[-1:]!r}, wanted the verdict it always printed"
    return None


def _printed_lines(root: Path) -> list[str]:
    """The lines a real run printed for `root`, so a case can read what it says.

    A run that reports a finding and a run that keeps it quiet print different words
    for the same tree, and an exit code is the same either way, so a case that says
    anything about the wording has to capture the wording. Both streams are kept,
    because the refusal and its summary go to one and the findings to the other.
    """
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer), contextlib.redirect_stderr(buffer):
        report(root, "HEAD", False)
    return buffer.getvalue().splitlines()


def _held_two_findings(surface: Surface, root: Path) -> str | None:
    """An entry naming no line waives neither of the two identical findings.

    Taking the first of them would leave the run green on a judgement about an
    assertion its author may never have read, and the guess would be invisible,
    because the count of waived findings would be the same either way.
    """
    if len(surface.stale_waivers) != 1:
        return f"{len(surface.stale_waivers)} entries called out, wanted the ambiguous one"
    if surface.waived:
        return f"waived {len(surface.waived)}, wanted neither of the two"
    if len(surface.findings) != 2:
        return f"left {len(surface.findings)} finding(s), wanted both"
    return None


def _held_two_recorded(surface: Surface, root: Path) -> str | None:
    """Both of two identical findings can be recorded, and both are then waived."""
    if surface.findings:
        return f"{len(surface.findings)} finding(s) left"
    if len(surface.waived) != 2:
        return f"waived {len(surface.waived)}, wanted both"
    if surface.stale_waivers:
        return f"{len(surface.stale_waivers)} entries called out, wanted none"
    return None


def _held_comparing(surface: Surface, root: Path) -> str | None:
    """A finding about a comparing macro can be recorded as the assertion reads.

    The claim of a comparing macro is two halves joined with a spaced `==`, and it
    is the one claim the reader has to normalise before comparing, or no entry for
    one of these can ever match.
    """
    if surface.findings:
        return f"{len(surface.findings)} finding(s) left"
    if len(surface.waived) != 1 or surface.waived[0].rule != "R1-changed":
        return f"waived {[finding.rule for finding in surface.waived]}, wanted the comparison"
    if surface.stale_waivers:
        return f"{len(surface.stale_waivers)} entries called out, wanted none"
    return None


def _held_other_rule(surface: Surface, root: Path) -> str | None:
    """A record silences the rule it names and nothing else on the line."""
    if len(surface.waived) != 1 or surface.waived[0].rule != "R1-changed":
        return f"waived {[finding.rule for finding in surface.waived]}, wanted the condition"
    return None


def _held_removal(surface: Surface, root: Path) -> str | None:
    """A removal is recordable, which is why it has a claim of its own."""
    if surface.findings:
        return f"{len(surface.findings)} finding(s) left"
    if len(surface.waived) != 1 or surface.waived[0].rule != "R1-removed":
        return f"waived {[finding.rule for finding in surface.waived]}, wanted the removal"
    return None


def _held_staged_list(surface: Surface, root: Path) -> str | None:
    """Staged mode reads the list from the index, and only from the index.

    Both runs are made against the same fixture with the same change, so the entry
    is honoured or ignored because of where it is and nothing else. A hook judging
    what is being committed has to see the entry that is being committed: an entry
    written to the disk and left there would pass now and fail once committed, by
    which time the person who wrote it is not watching.
    """
    honoured = scan(root, "HEAD", staged=True)
    if honoured.exit_code() != EXIT_OK or len(honoured.waived) != 1:
        return (
            f"a staged run over the index gave {honoured.exit_code()} with "
            f"{len(honoured.waived)} waived"
        )
    # The scratch directory is the parent every fixture is built in.
    ignored = scan(_staged_list_case(root.parent, "waiver-staged-disk", False), "HEAD", staged=True)
    if ignored.exit_code() != EXIT_FINDINGS or ignored.waived:
        return (
            f"an entry on the disk alone gave {ignored.exit_code()} with "
            f"{len(ignored.waived)} waived"
        )
    return None


#: What a case cannot say with its exit code, keyed by the case's own name and
#: consulted only once the exit code has been checked. An exit code says that
#: something was found and not what was found: which rule, on which line, which list
#: was read, and which of two identical findings was taken. A check returns None when
#: the case held, or a sentence saying what it saw instead, which is printed and fails
#: the case.
#:
#: A check is handed the surface and the fixture's root. The root is here because
#: the negative of one case is a second fixture that has to be built beside the
#: first, and a case that could only be judged against the repository it was handed
#: would have no negative at all.
def _held_index_without_list(surface: Surface, root: Path) -> str | None:
    """A list the index does not hold is refused in the run's own words.

    The exit code is the same 3 for anything this check could not read, an
    unresolvable base among it, so the code alone says nothing about the list. One
    entry, and that entry naming the list, is what says the refusal was about the
    list rather than about the repository around it.
    """
    if surface.exit_code() != EXIT_UNREADABLE:
        return f"the run gave {surface.exit_code()}"
    if len(surface.unreadable) != 1:
        return f"{len(surface.unreadable)} unreadable path(s), wanted only the list"
    if WAIVERS_PATH not in surface.unreadable[0]:
        return f"the refusal said {surface.unreadable[0]}"
    return None


MORE: dict[str, Callable[[Surface, Path], str | None]] = {
    "a finding a person recorded as intended is reported and waived": _held_matched,
    "a removed test function is reported for sign-off and fails nothing": (
        _held_removed_test
    ),
    "an entry for a file the run did not compare is reported and not failed": (
        _held_unexercised
    ),
    "a list that is not there is a failure, and the finding is still reported": (
        _held_absent
    ),
    "a list whose shape is wrong is a failure naming the list": _held_names_the_list,
    "a reason that is not a string is a failure naming the field, and nothing crashes": (
        _held_numbered_reason
    ),
    "a record naming a rule the check does not report is a failure naming it": (
        _held_unknown_rule
    ),
    "a record under one rule silences no finding of another that shares its first part": (
        _held_wrong_rule
    ),
    "a record the run could not judge on a run that compared nothing says so": (
        _held_nothing_compared
    ),
    "a source file that cannot be read is a failure about paths, not about the record": (
        _held_gone_source
    ),
    "an entry with no reason is a failure, not a record of nothing": (
        _held_names_the_list
    ),
    "an entry that matched nothing is a failure, and the finding stands": (
        _held_stale_text
    ),
    "an entry whose claim the file still asserts is spent, not a failure": (
        _held_spent
    ),
    "an entry whose claim has slid to another line stays a failure": (
        _held_slid_landed
    ),
    "an entry that matched two findings silences neither of them": (
        _held_two_findings
    ),
    "two findings in one file under one rule are both recorded": _held_two_recorded,
    "a record naming the second of two identical findings takes that one": (
        _held_second_line
    ),
    "the order two line-named records are written in does not decide which they take": (
        _held_both_lines
    ),
    "the claim a finding about an added assertion prints is the claim a record holds": (
        _held_printed_claim
    ),
    "the claim a removal finding prints is the claim a record holds": (
        _held_printed_claim
    ),
    "a green run names the recorded judgement inside its verdict": _held_verdict,
    "a finding about a comparing macro can be recorded as intended": _held_comparing,
    "an entry whose line is not the finding's line waives nothing": _held_slid_text,
    "an entry for one rule does not silence another rule on the same line": (
        _held_other_rule
    ),
    "a removed assertion can be recorded as intended": _held_removal,
    "staged mode reads the list from the index, not the disk": _held_staged_list,
    "a list the index does not hold is a failure, not an empty list": (
        _held_index_without_list
    ),
    "a list that writes one key twice at the top is a failure naming the key": (
        _held_repeated_top
    ),
    "a list that writes one path twice is a failure naming the layer": (
        _held_repeated_path
    ),
    "a recorded list that is a link is refused, not followed": _held_linked_list,
    "the same list is refused by the run that reads the index": _held_linked_list,
    "the printed contract is one line per kind, each beginning where the hook keeps it": (
        _held_printed_contract
    ),
    "the line about records the run did not judge is capped and counts the rest": (
        _held_capped
    ),
}


#: Text whose braces, parentheses and commas live inside literals and comments.
#: Each of these broke the mask at least once while this check was written: a
#: scanner that reads code out of a literal counts the literal's own brackets, and
#: a `,` inside a string splits a macro's arguments in the wrong place. They are
#: here so a reader does not have to take the claim on trust, and so the failure
#: has a fixture of its own rather than being found on someone's real file.
MASK_TRICKY = r'''
fn tricky(raw: &str) -> usize {
    // a comment with assert!( and a brace } and a paren (
    /* a block comment with "a string" and { and } */
    let literal = "a } brace, a { brace, and assert!(x)";
    let raw = r#"a raw "quote", a # and { } and assert!(x)"#;
    let hashed = r##"two # marks and " and { }"##;
    let byte = b'a';
    let escaped = "a quote \" and a backslash \\ and \{";
    let lifetime = 'a;
    let tick = '\'';
    let chars = ['a', '{', '}', ','];
    let comma = (1, 2, 3);
    assert_eq!(literal.len(), raw.len(), "{literal:?}");
    assert!(hashed.len() > 0, "{hashed:?}");
    assert_eq!(byte, b'a', "a byte: {byte:?}");
    assert_eq!(tick, b'\'', "a tick");
    assert_eq!(comma.0, 1, "the first: {comma:?}");
    raw.len()
}
'''


def _mask_cases(base: Path) -> list[tuple[str, int, Path, str]]:
    """Cases that judge the mask and the scanner on text built to defeat them."""
    return [
        (
            "brackets inside literals and comments are not code",
            EXIT_OK,
            _fixture(base, "mask-tricky", MASK_TRICKY, base_text=MASK_TRICKY),
            "mask",
        ),
        (
            "a brace inside a literal does not end the function early",
            EXIT_FINDINGS,
            _fixture(base, "mask-brace", MASK_TRICKY.replace(
                'assert_eq!(literal.len(), raw.len(), "{literal:?}");',
                'assert_eq!(raw.len(), literal.len(), "{literal:?}");',
            ), base_text=MASK_TRICKY),
            "R1-changed",
        ),
        (
            "an assert inside a comment is not an assertion",
            EXIT_OK,
            _fixture(base, "mask-comment", MASK_TRICKY.replace(
                "    let literal =",
                '    // assert!(nothing, "a commented-out assertion");\n    let literal =',
            ), base_text=MASK_TRICKY),
            "mask",
        ),
        (
            "an assert inside a string literal is not an assertion",
            EXIT_OK,
            _fixture(base, "mask-string", MASK_TRICKY.replace(
                '    let byte = b\'a\';',
                '    let text = "assert!(nothing, \\"not an assertion\\")";\n    let byte = b\'a\';',
            ), base_text=MASK_TRICKY),
            "mask",
        ),
    ]


def _cases(base: Path) -> list[tuple[str, int, Path, str]]:
    """(name, expected exit, fixture root, rule) for every exit-code case.

    Every rule appears as a pair: one fixture that must produce exactly that rule,
    and one that differs from it only in the offending construct being absent. A
    pair is the whole point, because a finding-free fixture proves nothing on its
    own -- it is equally consistent with a rule that never fires.

    The fourth field is the rule a run over the fixture has to report, so a case
    that fires more than the one it was built for fails rather than passing for the
    wrong reason. It is a word naming the case where a run has no finding to name a
    rule by. Anything else a case has to say is under its name in `MORE`, because a
    label read by a branch far below is not readable from the case that carries it.
    """
    messaged = 'assert!(fake.hide(), "the first hide must succeed: {budget:?}");'
    bare = 'assert!(fake.hide(), "the first hide must succeed");'
    return [
        # Rule 1: nothing removed, no condition changed.
        (
            "a message added to an assertion is a pass",
            EXIT_OK,
            _fixture(base, "r1-clean", MESSAGED),
            "R1",
        ),
        (
            "the same assertion wrapped over four lines is a pass",
            EXIT_OK,
            _fixture(base, "r1-reflowed", MESSAGED.replace(
                messaged,
                'assert!(\n'
                '                fake.hide(),\n'
                '                "the first hide: {budget:?}"\n'
                '            );',
            )),
            "R1",
        ),
        (
            "a removed assertion is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r1-removed", MESSAGED.replace(
                '            assert!(!fake.hide(), "a spent budget must not hide");\n', ""
            )),
            "R1-removed",
        ),
        (
            # The kind the base held vanishes entirely, so nothing is left to
            # group. Walking only the kinds present now made this invisible,
            # which is why the rule that most needed to fire was the one that
            # did not. One `assert_eq!` in its own function is the whole case.
            "removing the only assertion of a kind in a function is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "r1-removed-last",
                (CLEAN_BASE + ONLY_ONE_EQ).replace("        assert_eq!(1 + 1, 2);\n", ""),
                base_text=CLEAN_BASE + ONLY_ONE_EQ,
            ),
            "R1-removed",
        ),
        (
            "a changed condition is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r1-changed", MESSAGED.replace(messaged, messaged.replace(
                "assert!(fake.hide()", "assert!(!fake.hide()"
            ))),
            "R1-changed",
        ),
        (
            "a changed condition with no message added is still a finding",
            EXIT_FINDINGS,
            _fixture(base, "r1-changed-plain", CLEAN_BASE.replace(bare, bare.replace(
                "assert!(fake.hide()", "assert!(!fake.hide()"
            ))),
            "R1-changed",
        ),
        # Binding the value first is the change rule 2 asks for, so rule 1 has to
        # read it as the same claim. These two cases differ only in whether the
        # binding exists, and they must not give the same answer.
        (
            "a multi-line condition and the same one inlined are a pass",
            EXIT_OK,
            _fixture(base, "r1-inlined", MESSAGED.replace(
                messaged,
                'assert!(\n'
                '                fake\n'
                '                    .hide(),\n'
                '                "the first hide: {budget:?}"\n'
                '            );',
            )),
            "R1",
        ),
        (
            "the same condition bound to a let and written on one line is a pass",
            EXIT_OK,
            _fixture(base, "r1-chain-hoisted", MESSAGED.replace(
                "            let mut fake = Fake { budget: 1 };\n",
                "            let mut fake = Fake { budget: 1 };\n"
                "            let called = fake.hide();\n",
            ).replace(
                messaged,
                'assert!(called, "the first hide must succeed: {called:?}");',
            ).replace(
                "            let mut fake = Fake { budget: 1 };\n"
                "            let called = fake.hide();\n",
                "            let mut fake = Fake { budget: 1 };\n"
                "            let called = fake\n"
                "                .hide();\n",
            )),
            "R1",
        ),
        (
            "two assertions swapped round still assert what they asserted",
            EXIT_OK,
            _fixture(base, "r1-swapped", MESSAGED.replace(
                '            assert!(fake.hide(), "the first hide must succeed: {budget:?}");\n'
                '            assert!(!fake.hide(), "a spent budget must not hide");',
                '            assert!(!fake.hide(), "a spent budget must not hide");\n'
                '            assert!(fake.hide(), "the first hide must succeed: {budget:?}");',
            )),
            "R1",
        ),
        (
            "a swapped pair where one is negated is still a finding",
            EXIT_FINDINGS,
            _fixture(base, "r1-swapped-wrong", MESSAGED.replace(
                '            assert!(fake.hide(), "the first hide must succeed: {budget:?}");\n'
                '            assert!(!fake.hide(), "a spent budget must not hide");',
                '            assert!(!fake.hide(), "a spent budget must not hide");\n'
                '            assert!(!fake.hide(), "the first hide must succeed: {budget:?}");',
            )),
            "R1-changed",
        ),
        (
            "a two-step chain of bindings keeps the literal it was given",
            EXIT_OK,
            _fixture(base, "r1-chain-literal", CHAIN_LITERAL_WORK, base_text=CHAIN_LITERAL_BASE),
            "R1",
        ),
        (
            "a two-step chain of bindings is a pass",
            EXIT_OK,
            _fixture(base, "r1-chain", CHAIN_WORK, base_text=CHAIN_BASE),
            "R1",
        ),
        (
            "a bound value asserted by its own name is the same claim",
            EXIT_OK,
            _fixture(base, "r1-hoisted", MESSAGED.replace(
                "            let mut fake = Fake { budget: 1 };\n",
                "            let mut fake = Fake { budget: 1 };\n"
                "            let called = fake.hide();\n",
            ).replace(
                messaged,
                'assert!(called, "the first hide must succeed: {called:?}");',
            )),
            "R1",
        ),
        (
            "the same assertion with no binding is the base, and is clean too",
            EXIT_OK,
            _fixture(base, "r1-hoist-control", MESSAGED),
            "R1",
        ),
        (
            "a bound value asserted against a different claim is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r1-hoist-wrong", MESSAGED.replace(
                "            let mut fake = Fake { budget: 1 };\n",
                "            let mut fake = Fake { budget: 1 };\n"
                "            let called = fake.hide();\n",
            ).replace(
                messaged,
                'assert!(!called, "the first hide must succeed: {called:?}");',
            )),
            "R1-changed",
        ),
        # Rule 2: a message argument that calls something.
        (
            "a message argument that calls is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r2-call", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {:?}", fake.hide()',
            )),
            "R2-call",
        ),
        (
            "a message argument that calls a macro is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r2-macro-call", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {:?}", format!("{}", budget)',
            )),
            "R2-call",
        ),
        (
            "a message argument that indexes into a macro is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r2-bracket-macro", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {:?}", vec![budget][0]',
            )),
            "R2-call",
        ),
        (
            "the same value bound to a let first is a pass",
            EXIT_OK,
            _fixture(base, "r2-let", BOUND_BASE.replace(
                bare, 'assert!(first, "the first hide must succeed: {first:?}");'
            ), base_text=BOUND_BASE),
            "R2",
        ),
        (
            "a message argument that is a field path is a pass",
            EXIT_OK,
            _fixture(base, "r2-field", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {:?}", fake.budget',
            )),
            "R2",
        ),
        (
            "a parenthesis inside the message text is not a call",
            EXIT_OK,
            _fixture(base, "r2-paren-text", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed (the budget was one): {budget:?}"',
            )),
            "R2",
        ),
        # Rule 3: the format string itself.
        (
            "an inline capture naming a field is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r3-field-capture", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {fake.budget:?}"',
            )),
            "R3-capture",
        ),
        (
            "an inline capture naming an index is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r3-index-capture", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {budgets[0]:?}"',
            ).replace(
                "            let mut fake = Fake { budget: 1 };",
                "            let budgets = [1];\n            let mut fake = Fake { budget: 1 };",
            )),
            "R3-capture",
        ),
        (
            "a doubled brace in the message text is not a capture",
            EXIT_OK,
            _fixture(base, "r3-doubled", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed {{it is {budget:?}}} for {{budget}} items"',
            )),
            "R3",
        ),
        (
            "the same field passed as an argument is a pass",
            EXIT_OK,
            _fixture(base, "r3-field-arg", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {:?}", fake.budget',
            )),
            "R3",
        ),
        (
            "a bare identifier passed positionally is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r3-positional", MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {:?}", budget',
            ).replace(
                "            let mut fake = Fake { budget: 1 };",
                "            let budget = 1;\n            let mut fake = Fake { budget: 1 };",
            )),
            "R3-positional",
        ),
        (
            "the same identifier written into the format string is a pass",
            EXIT_OK,
            _fixture(base, "r3-inlined", MESSAGED),
            "R3",
        ),
        # Rule 4: an assertion where the base had none of that kind.
        (
            "an invented assertion in a function that had none is a finding",
            EXIT_FINDINGS,
            _fixture(base, "r4-invented", NO_ASSERT_BASE.replace(
                "            let hid = self.budget > 0;",
                "            let hid = self.budget > 0;\n"
                '            assert!(hid, "the fake must hide: {hid:?}");',
            ), base_text=NO_ASSERT_BASE),
            "R4-added",
        ),
        (
            "the same function with no added assertion is a pass",
            EXIT_OK,
            _fixture(base, "r4-absent", NO_ASSERT_BASE, base_text=NO_ASSERT_BASE),
            "R4",
        ),
        (
            "a new test function holding assertions is a pass",
            EXIT_OK,
            _fixture(base, "r4-new-fn", MESSAGED.replace(
                "        #[test]\n        fn spends_one_call() {",
                "        #[test]\n        fn starts_with_a_budget() {\n"
                "            let fake = Fake { budget: 1 };\n"
                '            assert_eq!(fake.budget, 1, "a new fake: {:?}", fake.budget);\n'
                "        }\n\n"
                "        #[test]\n        fn spends_one_call() {",
            )),
            "R4",
        ),
        (
            "a new file holding assertions is a pass",
            EXIT_OK,
            _new_file_case(base),
            "R4",
        ),
        (
            "a removed test function is reported for sign-off and fails nothing",
            EXIT_OK,
            _removed_test_case(base),
            "removed-test",
        ),
        # A base that does not resolve.
        (
            "a base that does not resolve is not a clean run",
            EXIT_UNREADABLE,
            _fixture(base, "base-missing", MESSAGED),
            "base",
        ),
        # The recorded judgements. Both of these cases pass on their exit code, so
        # the label says which list the case reads, and the run below reads it.
        (
            "a finding a person recorded as intended is reported and waived",
            EXIT_OK,
            _waived_budget_case(base),
            "waiver-matched",
        ),
        (
            "an entry for a file the run did not compare is reported and not failed",
            EXIT_OK,
            _unexercised_case(base),
            "waiver-unexercised",
        ),
        # The refusals. Each of these is a run whose answer is no, and each one has
        # something an exit code cannot say about it, which `MORE` says under the
        # case's name.
        (
            "a list that is not there is a failure, and the finding is still reported",
            EXIT_UNREADABLE,
            _absent_list_case(base),
            "R1-changed",
        ),
        (
            "a list whose shape is wrong is a failure naming the list",
            EXIT_UNREADABLE,
            _malformed_list_case(base),
            "R1-changed",
        ),
        (
            "an entry with no reason is a failure, not a record of nothing",
            EXIT_UNREADABLE,
            _reasonless_case(base),
            "R1-changed",
        ),
        (
            "a reason that is not a string is a failure naming the field, and nothing crashes",
            EXIT_UNREADABLE,
            _numbered_reason_case(base),
            "R1-changed",
        ),
        (
            "a record naming a rule the check does not report is a failure naming it",
            EXIT_UNREADABLE,
            _unknown_rule_case(base),
            "R1-changed",
        ),
        (
            "a record under one rule silences no finding of another that shares its first part",
            EXIT_FINDINGS,
            _wrong_rule_case(base),
            "R1-changed",
        ),
        (
            "a record the run could not judge on a run that compared nothing says so",
            EXIT_OK,
            _nothing_compared_case(base),
            "nothing-compared",
        ),
        (
            "a source file that cannot be read is a failure about paths, not about the record",
            EXIT_UNREADABLE,
            _unreadable_source_case(base),
            "unreadable",
        ),
        (
            "an entry that matched nothing is a failure, and the finding stands",
            EXIT_FINDINGS,
            _stale_entry_case(base),
            "R1-changed",
        ),
        (
            "an entry whose claim the file still asserts is spent, not a failure",
            EXIT_OK,
            _spent_entry_case(base),
            "R1-changed",
        ),
        (
            "an entry whose claim has slid to another line stays a failure",
            EXIT_FINDINGS,
            _slid_landed_case(base),
            "R1-changed",
        ),
        (
            "an entry that matched two findings silences neither of them",
            EXIT_FINDINGS,
            _two_findings_case(base),
            "R1-changed",
        ),
        (
            "two findings in one file under one rule are both recorded",
            EXIT_OK,
            _two_recorded_case(base),
            "R1-changed",
        ),
        (
            "a record naming the second of two identical findings takes that one",
            EXIT_FINDINGS,
            _second_line_case(base),
            "R1-changed",
        ),
        (
            "the order two line-named records are written in does not decide which they take",
            EXIT_OK,
            _reversed_lines_case(base),
            "R1-changed",
        ),
        (
            "the claim a finding about an added assertion prints is the claim a record holds",
            EXIT_OK,
            _added_comparing_case(base),
            "R4-added",
        ),
        (
            "the claim a removal finding prints is the claim a record holds",
            EXIT_OK,
            _printed_removal_case(base),
            "R1-removed",
        ),
        (
            "a green run names the recorded judgement inside its verdict",
            EXIT_OK,
            _verdict_case(base),
            "waiver-verdict",
        ),
        (
            "a finding about a comparing macro can be recorded as intended",
            EXIT_OK,
            _comparing_macro_case(base),
            "R1-changed",
        ),
        (
            "an entry whose line is not the finding's line waives nothing",
            EXIT_FINDINGS,
            _wrong_line_case(base),
            "R1-changed",
        ),
        (
            "an entry for one rule does not silence another rule on the same line",
            EXIT_FINDINGS,
            _other_rule_case(base),
            "R3-positional",
        ),
        (
            "a removed assertion can be recorded as intended",
            EXIT_OK,
            _recorded_removal_case(base),
            "R1-removed",
        ),
        (
            "staged mode reads the list from the index, not the disk",
            EXIT_FINDINGS,
            _staged_list_case(base, "waiver-staged", True),
            "R1-changed",
        ),
        (
            "a list the index does not hold is a failure, not an empty list",
            EXIT_UNREADABLE,
            _unstaged_list_case(base),
            "staged",
        ),
        (
            "a list that writes one key twice at the top is a failure naming the key",
            EXIT_UNREADABLE,
            _repeated_key_case(base, "waiver-repeat-top", REPEATED_TOP_LEVEL),
            "R1-changed",
        ),
        (
            "a list that writes one path twice is a failure naming the layer",
            EXIT_UNREADABLE,
            _repeated_key_case(base, "waiver-repeat-path", REPEATED_PATH),
            "R1-changed",
        ),
        (
            "a recorded list that is a link is refused, not followed",
            EXIT_UNREADABLE,
            _linked_list_case(base),
            "waiver-link",
        ),
        (
            "the same list is refused by the run that reads the index",
            EXIT_UNREADABLE,
            _list_directory_case(base),
            "staged",
        ),
        (
            "the printed contract is one line per kind, each beginning where the hook keeps it",
            EXIT_FINDINGS,
            _printed_contract_case(base),
            "R1-changed",
        ),
        (
            "the line about records the run did not judge is capped and counts the rest",
            EXIT_OK,
            _capped_list_case(base),
            "waiver-cap",
        ),
    ]


#: The base of the reporting case: five functions, each holding exactly one
#: assertion, so a defect in one of them cannot be blamed on another. Four of
#: them hold an `assert!` and one holds an `assert_eq!`, because rule 4 is about
#: a kind the function did not have, and a function that already held every kind
#: could not show it.
REPORT_BASE = """mod demo {
    struct Fake {
        budget: u32,
    }

    impl Fake {
        fn hide(&mut self) -> bool {
            self.budget = self.budget.saturating_sub(1);
            self.budget > 0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn doubled_call() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed");
        }

        #[test]
        fn capture_names_a_field() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed");
        }

        #[test]
        fn bare_identifier_passed() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed");
        }

        #[test]
        fn assertion_invented() {
            let mut fake = Fake { budget: 1 };
            assert_eq!(fake.budget, 1, "a new fake: {:?}", fake.budget);
        }

        #[test]
        fn condition_negated() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed");
        }

        #[test]
        fn left_alone() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed");
            assert!(!fake.hide(), "a spent budget must not hide");
        }
    }
}
"""

#: The same file with one defect in each of the four functions above and nothing
#: wrong in the sixth. Every change is confined to the message of an assertion
#: whose condition is untouched, so each reported line belongs to exactly one
#: rule.
REPORT_WORK = """mod demo {
    struct Fake {
        budget: u32,
    }

    impl Fake {
        fn hide(&mut self) -> bool {
            self.budget = self.budget.saturating_sub(1);
            self.budget > 0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn doubled_call() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed: {:?}", fake.hide());
        }

        #[test]
        fn capture_names_a_field() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed: {fake.budget:?}");
        }

        #[test]
        fn bare_identifier_passed() {
            let budget = 1;
            let mut fake = Fake { budget };
            assert!(fake.hide(), "the first hide must succeed: {:?}", budget);
        }

        #[test]
        fn assertion_invented() {
            let mut fake = Fake { budget: 1 };
            assert_eq!(fake.budget, 1, "a new fake: {:?}", fake.budget);
            let hid = fake.hide();
            assert!(hid, "the fake must hide: {hid:?}");
        }

        #[test]
        fn condition_negated() {
            let mut fake = Fake { budget: 1 };
            assert!(!fake.hide(), "the first hide must succeed: {budget:?}");
        }

        #[test]
        fn left_alone() {
            let mut fake = Fake { budget: 1 };
            assert!(fake.hide(), "the first hide must succeed");
            assert!(!fake.hide(), "a spent budget must not hide");
        }
    }
}
"""


def _finding_case(base: Path) -> Path:
    """A case that asserts the reported file and line, not only the exit code.

    An exit code cannot say which rule fired or where, and a run over one file
    cannot say which of four rules in it fired. So this fixture carries all four
    defects in four separate functions plus one function left correct, and the
    case asserts that exactly the four defective lines are reported and the fifth
    function is not.
    """
    return _fixture(base, "content", REPORT_WORK, base_text=REPORT_BASE)


#: The lines REPORT_WORK should be reported on, read from the file rather than
#: counted by hand, so a fixture edit cannot leave the expectation stale.
def _expected_lines(root: Path, needle_of: dict[str, str]) -> dict[str, int]:
    """{rule: line} for the given substrings in the fixture's working tree."""
    source = (root / FIXTURE_PATH).read_text(encoding="utf-8").splitlines()
    wanted: dict[str, int] = {}
    for rule, needle in needle_of.items():
        wanted[rule] = next(number for number, raw in enumerate(source, start=1) if needle in raw)
    return wanted


def _self_test() -> int:
    failures = 0
    total = 0
    with tempfile.TemporaryDirectory(prefix="assert-faithfulness-") as scratch:
        base = Path(scratch)
        reported: set[str] = set()
        total += 1
        failures += 0 if _normalise_case(base) else 1

        # The extra assertions are looked up by the case's own name, so a name that has
        # drifted from the case it belongs to turns them off and leaves the suite green.
        # Nothing else would notice, because the case is still there and still passing
        # on its exit code. Checked before the cases are judged, so a name naming no
        # case is a loud failure rather than silence.
        cases = _cases(base) + _mask_cases(base)
        total += 1
        named = {name for name, _, _, _ in cases}
        stray = sorted(name for name in MORE if name not in named)
        ok = not stray
        print(
            f"self-test {'ok' if ok else 'FAIL'}: every extra assertion names a case "
            f"(names naming no case: {stray})"
        )
        failures += 0 if ok else 1

        for name, want, root, rule in cases:
            total += 1
            try:
                if rule == "base":
                    surface = scan(root, "no-such-ref-here")
                elif rule == "staged":
                    # A fixture about the index has to be judged in the mode that reads
                    # it, or the case describes a run nobody makes.
                    surface = scan(root, "HEAD", staged=True)
                else:
                    surface = scan(root, "HEAD")
            except Exception as error:  # noqa: BLE001
                # A fixture that makes the check raise is a defect, and the whole point
                # of running every fixture is being told which one. Letting the exception
                # out ends the suite with a traceback naming a line number in this file
                # and no case at all, so the reader is left working out which fixture
                # they stopped at. A refusal is a return, not a raise, so this only
                # catches a check that fell over rather than one that said no.
                print(
                    f"self-test FAIL: {name} made the check raise "
                    f"{type(error).__name__}: {error}"
                )
                failures += 1
                continue
            got = surface.exit_code()
            ok = got == want
            if ok and want == EXIT_FINDINGS and rule not in ("base",):
                # An exit code of 1 says something was found and not what. A
                # fixture built for one rule that trips another has proved nothing
                # about the rule its name claims, which is how a case passes for
                # the wrong reason.
                found = {finding.rule for finding in surface.findings}
                ok = found == {rule}
                if not ok:
                    print(f"self-test: {name} produced {sorted(found)}, wanted ['{rule}']")
            if ok and name in MORE:
                # The exit code cannot say which list was read, which line a record
                # named, or whether a finding was judged and recorded rather than
                # dropped on the floor, so a case that has more than one thing to
                # prove says so under its own name.
                problem = MORE[name](surface, root)
                ok = problem is None
                if problem is not None:
                    print(f"self-test: {name}: {problem}")
            if ok and want == EXIT_FINDINGS:
                reported.update(finding.rule for finding in surface.findings)
            print(f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want})")
            failures += 0 if ok else 1

        # Every rule has to be seen failing somewhere, or the exit-code cases
        # above only proved that the verdict moves. The rule each case declares
        # is also asserted against what the run actually reported, so a case whose
        # fixture trips a different rule fails instead of passing. The names come
        # from the one list the reader refuses against, because a second copy of them
        # here would let a rule be emitted and never recorded, or recorded and never
        # emitted, with nothing failing either way.
        total += 1
        want_rules = RULE_IDS
        missing = sorted(want_rules - reported)
        stray = sorted(reported - want_rules)
        ok = not missing and not stray
        print(
            f"self-test {'ok' if ok else 'FAIL'}: every rule was produced by the fixture "
            f"built for it, and the check reports nothing else (missing {missing}, "
            f"reported that is not a rule {stray})"
        )
        failures += 0 if ok else 1

        # The exit code cannot say which rule fired or on which line, so this
        # asserts the findings themselves against a whole fixture tree. Each rule
        # is looked up by the text of the line it should land on, so the
        # expectation cannot drift when the fixture is edited.
        total += 1
        root = _finding_case(base)
        surface = scan(root, "HEAD")
        want = _expected_lines(
            root,
            {
                "R2-call": 'must succeed: {:?}", fake.hide()',
                "R3-capture": "{fake.budget:?}",
                "R3-positional": 'must succeed: {:?}", budget',
                "R4-added": "the fake must hide",
                "R1-changed": 'assert!(!fake.hide(), "the first hide must succeed',
            },
        )
        got = {finding.rule: (finding.path, finding.line) for finding in surface.findings}
        want_where = {rule: (FIXTURE_PATH, line) for rule, line in want.items()}
        ok = got == want_where and surface.exit_code() == EXIT_FINDINGS
        print(
            f"self-test {'ok' if ok else 'FAIL'}: each rule is reported at its own line in a "
            f"file holding all four (reported {sorted(got.items())}, "
            f"want {sorted(want_where.items())})"
        )
        failures += 0 if ok else 1

        # The same fixture, unchanged, has to be clean. Without this the case
        # above would also pass against a check that reports every line.
        total += 1
        clean_root = _fixture(base, "content-clean", None, base_text=REPORT_BASE)
        clean_surface = scan(clean_root, "HEAD")
        ok = clean_surface.exit_code() == EXIT_OK and not clean_surface.findings
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the reporting fixture with no change is clean "
            f"(exit {clean_surface.exit_code()}, {len(clean_surface.findings)} finding(s))"
        )
        failures += 0 if ok else 1

        # Staged mode has to read the index and not the disk, or a commit hook
        # judges whatever happened to be on disk. The fixture puts the defect in
        # the index and then puts the base back on disk, so a run that read the
        # working tree would find nothing.
        total += 1
        staged_root = _fixture(
            base,
            "staged",
            MESSAGED.replace(
                '"the first hide must succeed: {budget:?}"',
                '"the first hide must succeed: {fake.budget:?}"',
            ),
            staged=True,
        )
        surface = scan(staged_root, "HEAD", staged=True)
        staged_rules = [finding.rule for finding in surface.findings]
        ok = staged_rules == ["R3-capture"] and surface.exit_code() == EXIT_FINDINGS
        print(
            f"self-test {'ok' if ok else 'FAIL'}: staged mode reads the index, not the "
            f"working tree (reported {staged_rules}, want ['R3-capture'])"
        )
        failures += 0 if ok else 1

    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total} cases)")
    return EXIT_OK


# --- Entry point ----------------------------------------------------------------


def _parse(argv: list[str]) -> tuple[str, bool] | None:
    """Read `--base` and `--staged`, or None when the arguments make no sense."""
    base = DEFAULT_BASE
    staged = False
    index = 0
    while index < len(argv):
        argument = argv[index]
        if argument == "--staged":
            staged = True
            index += 1
            continue
        if argument == "--base" and index + 1 < len(argv):
            base = argv[index + 1]
            index += 2
            continue
        if argument.startswith("--base=") and len(argument) > len("--base="):
            base = argument[len("--base=") :]
            index += 1
            continue
        return None
    if not base.strip():
        return None
    return base, staged


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return EXIT_OK
    root = Path(__file__).resolve().parent.parent
    if "--list-scanned" in argv:
        rest = [argument for argument in argv if argument != "--list-scanned"]
        parsed = _parse(rest)
        if parsed is None:
            print(USAGE, file=sys.stderr)
            return EXIT_USAGE
        for relative in scan(root, *parsed).scanned:
            print(relative)
        return EXIT_OK
    parsed = _parse(argv)
    if parsed is None:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    return report(root, *parsed)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))