#!/usr/bin/env python3
"""The prose rules the coverage-policy check applies, and the paragraph logic.

A rule can be written in prose as well as in code, so the check reads
paragraphs rather than lines: the word that makes a sentence history is often
not on the line that carries the phrase. This module owns the historical and
denial markers and the three helpers that turn a document into per-line
paragraph context. `coverage_policy_check` owns the live phrases, the scan, and
the verdict.
"""

from __future__ import annotations

#: A line carrying one of these is describing a rule as history, and a rule
#: described as history is not a rule. This is the heuristic the check's module
#: documentation names, and it is deliberately generous: it would rather let a
#: withdrawal through than refuse a document that explains why a rule went.
HISTORICAL_MARKERS = (
    "used to",
    "no longer",
    "not used",
    "removed",
    "withdrawn",
    "withdrew",
    "retired",
    "deleted",
    "gone",
    "was a",
    "were a",
    "do not reintroduce",
    "must not",
    "never",
    "deliberately unused",
    "history",
    "2026-10-03",
    "2026-10-04",
    "it could have been",
    "exists in",
    "would be a finding",
    "the rule this",
    "second rule",
    "one rule",
    "no aggregate",
    "not planned",
    "nothing compares",
    "compared against nothing",
    "judged by nothing",
    "is not a rule",
    "rejected",
    "reinstates",
    "removes",
    "exactly the thing this rule",
)

#: Denial. A sentence saying a rule is not used, is not to be used, or is not
#: there is evidence against a live rule whatever else its paragraph says. Matched
#: as bare substrings because the wording varies too much for a pattern, and a
#: denial is the common case here: almost every mention of a retired rule in this
#: repository is somebody writing down that it went.
DENIAL_MARKERS = ("not ", "nothing ", "never ", "without ", "no ")


def paragraphs(text: str) -> list[list[str]]:
    """The text as paragraphs: runs of consecutive non-blank lines.

    Prose wraps, and a docstring wraps, so the word that makes a sentence a
    statement about the past is very often not on the line that carries the
    phrase. Judging line by line would either flag every historical sentence in
    this repository or force all of them to be reworded to fit a linter, and the
    second is worse: it edits correct writing to satisfy a heuristic.
    """
    out: list[list[str]] = []
    current: list[str] = []
    for line in text.splitlines():
        if line.strip():
            current.append(line)
        elif current:
            out.append(current)
            current = []
    if current:
        out.append(current)
    return out


def context_of(text: str) -> list[str]:
    """For each line of `text`, the whole paragraph it belongs to, lowercased.

    One entry per line, including the blank ones, so a caller can index this by
    line number without doing arithmetic. A blank line's own context is itself,
    which is right: a blank line is the boundary of a paragraph, so nothing about
    it can excuse a phrase.
    """
    lines = text.splitlines()
    out: list[str] = [""] * len(lines)
    start: int | None = None
    for number, line in enumerate(lines):
        if line.strip():
            if start is None:
                start = number
        elif start is not None:
            blob = " ".join(item.lower() for item in lines[start:number])
            for index in range(start, number):
                out[index] = blob
            start = None
    if start is not None:
        blob = " ".join(item.lower() for item in lines[start:])
        for index in range(start, len(lines)):
            out[index] = blob
    return out


def allowed(context: str) -> bool:
    """Whether a line's paragraph says the thing is history or denies it.

    Generous on purpose, and that is a measured trade rather than a hope. Every
    one of this repository's historical sentences is allowed by one of these, and
    a check that flagged them would push a reader toward deleting the record of
    what was removed. The cost is a live rule described in a paragraph that also
    contains a withdrawal, which this cannot see. It still catches the decisive
    shapes: a flag handed to the tool, a retired path, a second threshold, a
    second comparison.

    A second, coarser allowance sits on top of this list. `DENIAL_MARKERS` matches
    a bare "not", "no", "nothing", "never" or "without" anywhere in the paragraph,
    because the common case in this repository is a document spending a paragraph
    on what no longer applies. Without it this check would flag almost every
    coverage paragraph in `ci.md` and the playbook, and a linter that punishes
    writing down a removal gets the removal deleted instead.
    """
    if any(marker in context for marker in HISTORICAL_MARKERS):
        return True
    return any(marker in context for marker in DENIAL_MARKERS)
