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
    }
)

#: Suffixes worth reading. Prose and build files, not binaries and not fixtures.
SCANNED_SUFFIXES = frozenset({".py", ".md", ".yml", ".yaml", ".toml", ".json", ".rs"})
SCANNED_NAMES = frozenset({"Makefile", "Dockerfile", "justfile"})

#: Files too large to be about a rule, and generated ones.
SKIP_NAMES = frozenset({"Cargo.lock", "coverage_report_fixture.json"})


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

#: A line carrying one of these is describing a rule as history, and a rule
#: described as history is not a rule. This is the heuristic the module
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


#: Denial. A sentence saying a rule is not used, is not to be used, or is not
#: there is evidence against a live rule whatever else its paragraph says. Matched
#: as bare substrings because the wording varies too much for a pattern, and a
#: denial is the common case here: almost every mention of a retired rule in this
#: repository is somebody writing down that it went.
DENIAL_MARKERS = ("not ", "nothing ", "never ", "without ", "no ")


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
        # This file names every shape it looks for, so it would find all of them
        # in itself. It is the definition of the rules, not an instance of them.
        if rel == "scripts/coverage_policy_check.py":
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


def self_test() -> int:
    """The rules, on inputs that cannot drift."""
    failures = 0

    def expect(name: str, got: object, want: object) -> None:
        nonlocal failures
        if got != want:
            failures += 1
            print(f"self-test FAIL: {name}: got {got!r}, want {want!r}")
        else:
            print(f"self-test ok: {name}")

    def with_files(files: dict[str, str]):
        """A tree of `files`, as text, checked without touching the real one."""
        class Fake:
            def exists(self) -> bool:
                return False

            def read_text(self, encoding: str = "utf-8") -> str:
                return ""

        findings: list[Finding] = []
        for retired in RETIRED_PATHS:
            if retired in files:
                findings.append(Finding("a retired mechanism is back", retired, 0, "exists again"))
        for rel, text in files.items():
            if rel == "scripts/coverage_policy_check.py":
                continue
            findings += scan_text("flag", rel, text, ((THRESHOLD_FLAG.pattern, "flag"),))
            findings += prose_findings(rel, text)
            if rel != GATE and not rel.endswith(".rs"):
                findings += scan_text("floor", rel, text, ((FLOOR_ASSIGNMENT.pattern, "floor"),))
                findings += scan_text("cmp", rel, text, ((COMPARISON.pattern, "cmp"),))
        return findings

    # A clean tree is clean. The gate as it stands, with its own comparison.
    clean = with_files({GATE: f"{EXPECTED_FLOOR}\nif row.lines_pct < floor:\n    pass\n"})
    expect("a tree holding only the rule is clean", [f.rule for f in clean], [])

    # A threshold flag anywhere is a finding, every family of it.
    for flag in ("--fail-under-lines", "--fail-under-functions", "--fail-under-file-lines", "--fail-uncovered-regions"):
        expect(f"{flag} is a finding", bool(with_files({"Makefile": f"cov {flag} 80\n"})), True)
    expect("a flag inside prose about history is not a finding",
           bool(with_files({"ci.md": f"It used to carry {chr(96)}--fail-under-lines{chr(96)} 80.\n"})), False)

    # A retired mechanism, by path and by name.
    expect("a retired path is a finding", bool(with_files({"scripts/coverage_guard.py": ""})), True)
    expect("a retired floors file is a finding", bool(with_files({"scripts/coverage_floors.json": "{}"})), True)

    # A second threshold, and a second comparison, in a file that is not the gate.
    expect("a second floor constant is a finding", bool(with_files({"scripts/other.py": "MIN_COVERAGE = 90\n"})), True)
    expect("a second comparison site is a finding",
           bool(with_files({"scripts/other.py": "if row.lines_pct >= 80:\n"})), True)
    expect("the gate's own comparison is not a finding", bool(with_files({GATE: "if row.lines_pct < floor:\n"})), False)
    expect("a threshold in a makefile is a finding",
           bool(with_files({"Makefile": "MIN_COVERAGE = 90\n"})), True)
    expect("a threshold withdrawn in a comment is not a finding",
           bool(with_files({"Makefile": "# it used to carry --fail-under-lines 80\n"})), False)
    expect("Rust domain vocabulary is not a coverage threshold",
           bool(with_files({"crates/clio-retrieve/src/floors.rs": "const FLOOR: f64 = 0.5;\n"})), False)
    expect("a coverage rule described in a Rust comment is still found",
           bool(with_files({"crates/a/src/x.rs": "// every file must reach 80% of its functions\n"})), True)

    # Prose. A live phrase is a finding; the same phrase as history is not.
    live = [
        ("a changed file must reach 80% of its functions", "functions must reach"),
        ("the aggregate floor is 80%", "aggregate floor"),
        ("every file must clear the added-line floor", "added-line floor"),
        ("see the recorded floors for this file", "recorded floors"),
        ("run coverage-guard to check", "coverage-guard live"),
    ]
    for text, name in live:
        expect(f"prose reading as a live rule: {name}", bool(with_files({"docs/x.md": text + "\n"})), True)
    history = [
        "It used to carry a function floor, which was withdrawn on 2026-10-03.",
        "The aggregate floor was removed; nothing compares the total now.",
        "Do not reintroduce --fail-under-regions without a human decision.",
        "The recorded floors are gone; the file was deleted.",
        "`coverage-guard` is deleted and must not be reintroduced.",
        "There is no aggregate requirement and no requirement on untouched files.",
    ]
    for text in history:
        expect(f"prose describing history is not a finding: {text[:44]!r}", bool(with_files({"docs/x.md": text + "\n"})), False)

    # The check exempts itself, because it has to name every shape it looks for.
    expect("this file is exempt from the prose rule",
           bool(with_files({"scripts/coverage_policy_check.py": "a function floor appears here\n"})), False)

    # The gate's own two invariants, checked against the real file.
    try:
        gate_text = (ROOT / GATE).read_text(encoding="utf-8")
    except OSError as error:
        expect("the gate is readable", str(error), "readable")
        gate_text = ""
    expect("the gate defines the one threshold exactly once",
           len(re.findall(r"^LINE_FLOOR\s*=", gate_text, re.MULTILINE)), 1)
    expect("the gate states the number the rule states", EXPECTED_FLOOR in gate_text, True)
    expect("the gate holds the one comparison once",
           len(re.findall(re.escape(EXPECTED_COMPARISON), gate_text)), 1)

    # The real tree, which is the assertion that matters.
    found, unread = scan()
    expect("no file this check needs is unreadable", unread, [])
    expect("the real tree holds one rule", [(f.rule, f.path, f.line) for f in found], [])

    print(f"self-test: {'all passed' if failures == 0 else f'{failures} failed'}")
    return EXIT_OK if failures == 0 else 1


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