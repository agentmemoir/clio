#!/usr/bin/env python3
"""Refuse a second coverage rule, in code or in prose.

This repository has one coverage requirement: every Rust file a change creates or
modifies must reach 80% of its lines. Four other rules were removed on 2026-10-04
— a per-file recorded floor, a floor on the lines a diff added, a comparison of
the report's total against a stored baseline, and a whole-report judge — and the
reason they went is that each was a second rule rather than a detail of the first.
Nothing in the tree stops the next one from arriving, which is how the four got
there: each was added in a change that was about something else.

So this check states the rule as a set of shapes that must not appear, and refuses
the tree when one does. It covers three things, each decidable without reading
prose as prose:

1. **A threshold flag passed to the coverage tool.** Any `--fail-under-*` or
   `--fail-uncovered-*` argument in a Makefile, a workflow or a script is a
   finding. The tool has those flags, including a per-file one, and every one of
   them gates something other than this rule: a total, or every file in a report
   rather than the files a change touched.

2. **A file or a target that used to carry a second rule.** The removed
   mechanisms are named in a closed list. If one comes back under its own name,
   it is a finding whatever it now does.

3. **A second place a coverage number is compared.** `LINE_FLOOR` must be defined
   once, in one file, and the only comparison of a line percentage against it must
   be in that file's `judge`. A second constant named like a floor, or a second
   comparison site, is a finding.

**One thing to know about what it reads.** It walks the working tree, so it reads
the root `AGENTS.md`, which is a symlink into the private repository that is not
in git. On a developer machine that means the private agent rules are checked
here too, which is useful. On a CI runner the symlink dangles, is not a file, and
is skipped, so the same check covers a slightly smaller set of documents. Nothing
fails either way; the set is printed by `--list-scanned` when it matters.

**What this is not.** It is a lint over a closed list of shapes, not a proof that
the tree holds one rule. It cannot tell a document that describes a withdrawn
rule as history from one that describes it as current, so prose is checked
separately and loosely: a phrase that can only mean a live second rule is a
finding unless the same line says the rule is gone. That allowance is a
heuristic and it will admit a history written without a past-tense marker. It is
here to catch a reintroduced rule, not to certify the absence of one.

Usage:
  python3 scripts/coverage_policy_check.py
  python3 scripts/coverage_policy_check.py --self-test
  python3 scripts/coverage_policy_check.py --list-scanned

Exit codes:
  0  no second rule is present
  1  at least one is
  2  usage error
  3  a file this check must read could not be read (fail closed)
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Iterable, NamedTuple

from coverage_policy_prose import allowed, context_of

ROOT = Path(__file__).resolve().parent.parent

#: Where the rule lives. The gate script, and nothing else.
GATE = "scripts/scoped_runner.py"

#: Directories that hold no rule, so a word in them cannot be one.
SKIP_DIRS = frozenset(
    {
        ".git",
        "target",
        "node_modules",
        "private",
        "logs",
        "__pycache__",
        "coverage",
        "html",
        ".cache",
    }
)

#: Suffixes worth reading. Prose and build files, not binaries and not fixtures.
SCANNED_SUFFIXES = frozenset({".py", ".md", ".yml", ".yaml", ".toml", ".json", ".rs", ".mk"})
SCANNED_NAMES = frozenset({"Makefile", "Dockerfile", "justfile"})

#: Files too large to be about a rule, and generated ones.
SKIP_NAMES = frozenset({"Cargo.lock", "coverage_report_fixture.json"})

#: Files that name every shape this check looks for, so it would find all of
#: them in them. They are the definition of the rules, not an instance of them.
SELF_FILES = frozenset(
    {
        "scripts/coverage_policy_check.py",
        "scripts/coverage_policy_selftest.py",
    }
)


class Finding(NamedTuple):
    """One thing that would make a second rule present, and where it was read."""

    rule: str
    path: str
    line: int
    detail: str


# --- 1. a threshold flag handed to the coverage tool ------------------------------

#: Any argument that makes the tool itself fail a run on a coverage number. Both
#: families are here: the `fail-under` ones gate a percentage, the `fail-uncovered`
#: ones gate a count, and a count is a different metric rather than a stricter
#: version of the same one.
THRESHOLD_FLAG = re.compile(r"--fail-(?:under|uncovered)-[a-z-]+")

# --- 2. a removed mechanism, by name ----------------------------------------------

#: Closed list. A path is a finding because it exists at all.
RETIRED_PATHS = (
    "scripts/coverage_floors.json",
    "scripts/coverage_guard.py",
    "scripts/coverage_scoped.py",
    "hooks.d/10-coverage-guard-selftest.py",
)

#: A make target or script name that used to carry a second rule.
RETIRED_NAMES = (
    "coverage-guard",
    "scripts-check-coverage-guard",
)

# --- 3. a second comparison or a second constant ---------------------------------

#: An assignment to something named like a coverage threshold, anywhere.
FLOOR_ASSIGNMENT = re.compile(
    r"^\s*(?:[A-Z_]*FLOOR[A-Z_]*|FLOOR|THRESHOLD|MIN_COVERAGE|COVERAGE_MIN)\s*[:=]",
    re.IGNORECASE | re.MULTILINE,
)

#: A line percentage compared with anything. The gate's own comparison is exempt,
#: and so is this file, which has to name the shape in order to find it.
COMPARISON = re.compile(r"lines_pct\s*(?:<|>|<=|>=|==|!=)\s*\w+")

#: The one definition the rule allows.
EXPECTED_FLOOR = "LINE_FLOOR = 80.0"

#: The one comparison the rule allows, quoted as it is written in the gate.
EXPECTED_COMPARISON = "row.lines_pct < floor"


# --- prose ------------------------------------------------------------------------

#: Phrases that can only describe a live coverage criterion other than line
#: coverage per changed file. Each is checked case-insensitively.
LIVE_PHRASES = (
    (r"function(?:s|al)? (?:coverage )?floor", "a function-coverage floor"),
    (r"(?:must|shall|has to|need to|required to)[^.]{0,40}function\s+coverage", "a requirement on function coverage"),
    (r"function\s+coverage[^.]{0,30}(?:must|shall|required|floor|gate|reach)", "a requirement on function coverage"),
    (r"\d+\s*%\s*of\s+(?:its\s+|their\s+)?functions?\b", "a percentage of functions as a requirement"),
    (r"functions?\s*(?:and|,)\s*lines", "a two-metric requirement"),
    (r"region(?:s|al)? (?:coverage )?floor", "a region-coverage floor"),
    (r"branch coverage (?:floor|gate|require)", "a branch-coverage gate"),
    (r"statement coverage", "a statement-coverage criterion"),
    (r"aggregate (?:coverage )?floor", "an aggregate floor"),
    (r"workspace (?:total|aggregate) floor", "an aggregate floor"),
    (r"added[- ]line (?:coverage )?(?:floor|gate)", "a floor on the lines a diff added"),
    (r"recorded (?:per-file )?floors?(?: file)?", "a recorded per-file floor"),
    (r"regression baseline", "an aggregate regression baseline"),
    (r"coverage-guard", "the whole-report judge"),
    (r"coverage_floors\.json", "the floors file"),
    (r"--fail-under-(?:lines|functions|regions|file-lines)", "a threshold flag"),
)


def candidate_paths(root: Path = ROOT) -> list[Path]:
    """Every file this check reads, in a stable order.

    Sorted so two runs over one tree read the same set in the same sequence, which
    is what makes a finding reproducible from a log line alone.
    """
    out: list[Path] = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        rel = path.relative_to(root)
        if set(rel.parts) & SKIP_DIRS:
            continue
        if rel.name in SKIP_NAMES or path.suffix in SKIP_NAMES:
            continue
        if path.name in SCANNED_NAMES or path.suffix in SCANNED_SUFFIXES:
            out.append(path)
    return sorted(out)


def scan_text(rule: str, rel: str, text: str, patterns: Iterable[tuple[str, str]]) -> list[Finding]:
    """Every match of `patterns` in `text` that is not describing a withdrawal.

    The allowance is the same one the prose scan uses, and it is applied here for
    the same reason: a comment that says a flag was used to be there is evidence
    the rule was removed, not evidence it is live. It is deliberately generous,
    and it is the reason a flag inside an explanatory comment is allowed while the
    same flag in a recipe is not.
    """
    out: list[Finding] = []
    context = context_of(text)
    for pattern, detail in patterns:
        for number, line in enumerate(text.splitlines(), 1):
            if not re.search(pattern, line, re.IGNORECASE):
                continue
            if allowed(context[number - 1]):
                continue
            out.append(Finding(rule, rel, number, detail))
    return out


def prose_findings(rel: str, text: str) -> list[Finding]:
    """A live phrase is a finding; the same phrase in the past tense is not.

    The allowance is per line rather than per document on purpose: a document that
    explains a withdrawal usually names the thing it withdrew, on the line that
    says it was withdrawn, and refusing that would make the explanation itself
    unfalsifiable.
    """
    out: list[Finding] = []
    context = context_of(text)
    for line_number, line in enumerate(text.splitlines(), 1):
        lowered = line.lower()
        if allowed(context[line_number - 1]):
            continue
        for pattern, detail in LIVE_PHRASES:
            if re.search(pattern, lowered):
                out.append(
                    Finding("a phrase that reads as a live second rule", rel, line_number, detail)
                )
    return out


def scan(root: Path = ROOT) -> tuple[list[Finding], list[str]]:
    """Every finding in the tree, and every path that could not be read.

    A path this check must read and cannot is returned rather than skipped, and
    the caller fails closed on it. A coverage rule that hides behind a file nobody
    can read is the case this whole exercise exists to catch.
    """
    findings: list[Finding] = []
    unreadable: list[str] = []

    for retired in RETIRED_PATHS:
        if (root / retired).exists():
            findings.append(
                Finding("a retired mechanism is back", retired, 0, f"{retired} exists again")
            )

    for path in candidate_paths(root):
        rel = str(path.relative_to(root))
        # These files name every shape the check looks for, so it would find
        # all of them in them. They are the definition of the rules, not an
        # instance of them.
        if rel in SELF_FILES:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            unreadable.append(f"{rel}: {error}")
            continue
        findings += scan_text(
            "a threshold flag handed to the tool",
            rel,
            text,
            ((THRESHOLD_FLAG.pattern, "a threshold flag the coverage tool would enforce"),),
        )
        findings += prose_findings(rel, text)
        # Rust source is excluded from the code-shape scans, and that is a measured
        # decision rather than a gap: this repository has a `crates/clio-retrieve/
        # src/floors.rs` about memory-retention floors, and `retention.rs` beside
        # it, so a scan for a name like `FLOOR` over product code reports domain
        # vocabulary as a coverage rule. The flag and phrase scans still read Rust,
        # so a comment in a product file describing a second coverage rule is
        # still found.
        if rel != GATE and path.suffix != ".rs":
            findings += scan_text(
                "a second coverage threshold",
                rel,
                text,
                ((FLOOR_ASSIGNMENT.pattern, "something named like a coverage threshold"),),
            )
            findings += scan_text(
                "a second place a line percentage is compared",
                rel,
                text,
                ((COMPARISON.pattern, "a line percentage compared with something"),),
            )

    # The gate itself has to hold exactly one of each.
    gate_path = root / GATE
    try:
        gate = gate_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        unreadable.append(f"{GATE}: {error}")
        return findings, unreadable
    definitions = len(re.findall(r"^LINE_FLOOR\s*=", gate, re.MULTILINE))
    if definitions != 1:
        findings.append(
            Finding("the one threshold is not defined exactly once", GATE, 0, f"{definitions} definitions of LINE_FLOOR, want 1")
        )
    elif EXPECTED_FLOOR not in gate:
        findings.append(
            Finding("the one threshold is not the number the rule states", GATE, 0, f"expected {EXPECTED_FLOOR!r}")
        )
    comparisons = len(re.findall(re.escape(EXPECTED_COMPARISON), gate))
    if EXPECTED_COMPARISON not in gate:
        findings.append(
            Finding("the one comparison is not written as the rule states", GATE, 0, f"expected {EXPECTED_COMPARISON!r}")
        )
    elif comparisons > 1:
        findings.append(
            Finding("the one comparison appears more than once", GATE, 0, f"{comparisons} occurrences")
        )
    return findings, unreadable


EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2
EXIT_ERROR = 3


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Refuse a second coverage rule, in code or in prose."
    )
    parser.add_argument("--self-test", action="store_true", help="run this script's own tests")
    parser.add_argument("--list-scanned", action="store_true", help="print every path this check reads")
    args = parser.parse_args(argv)
    if args.self_test:
        from coverage_policy_selftest import self_test

        return self_test()
    if args.list_scanned:
        for path in candidate_paths():
            print(path.relative_to(ROOT))
        return EXIT_OK
    findings, unreadable = scan()
    for finding in findings:
        where = f"{finding.path}:{finding.line}" if finding.line else finding.path
        print(f"coverage-policy-check: FINDING {where}: {finding.rule} ({finding.detail})")
    for path in unreadable:
        print(f"coverage-policy-check: COULD NOT READ {path}", file=sys.stderr)
    if unreadable:
        print(
            f"coverage-policy-check: {len(unreadable)} path(s) this check must read "
            "could not be read, so the tree is unchecked rather than clean",
            file=sys.stderr,
        )
        return EXIT_ERROR
    if findings:
        print(
            f"coverage-policy-check: {len(findings)} finding(s): the tree describes or "
            "enforces a coverage rule other than 80% of lines per created or "
            "modified file",
            file=sys.stderr,
        )
        return EXIT_FAIL
    print(
        "coverage-policy-check: the tree holds one coverage rule: 80% of lines on "
        "every Rust file a change creates or modifies, compared in one place"
    )
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))