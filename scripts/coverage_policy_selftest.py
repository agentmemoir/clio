#!/usr/bin/env python3
"""The coverage-policy check's own tests, on inputs that cannot drift.

Kept beside the check because it names every shape the check looks for, which
makes it an instance of the very rules the check refuses. It is exempt for the
same reason `coverage_policy_check` is.
"""

from __future__ import annotations

import re
import tempfile
from fnmatch import fnmatch
from pathlib import Path

from coverage_policy_check import (
    COMPARISON,
    EXPECTED_COMPARISON,
    EXPECTED_FLOOR,
    FLOOR_ASSIGNMENT,
    GATE,
    GATE_SAMPLE,
    RETIRED_PATHS,
    ROOT,
    SELF_FILES,
    THRESHOLD_FLAG,
    gate_text,
    Finding,
    candidate_paths,
    prose_findings,
    scan,
    scan_text,
)


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
            if rel in SELF_FILES:
                continue
            findings += scan_text("flag", rel, text, ((THRESHOLD_FLAG.pattern, "flag"),))
            findings += prose_findings(rel, text)
            if not fnmatch(rel, GATE) and not rel.endswith(".rs"):
                findings += scan_text("floor", rel, text, ((FLOOR_ASSIGNMENT.pattern, "floor"),))
                findings += scan_text("cmp", rel, text, ((COMPARISON.pattern, "cmp"),))
        return findings

    # A clean tree is clean. The gate as it stands, with its own comparison.
    clean = with_files({GATE_SAMPLE: f"{EXPECTED_FLOOR}\nif row.lines_pct < floor:\n    pass\n"})
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
    expect("the gate's own comparison is not a finding", bool(with_files({GATE_SAMPLE: "if row.lines_pct < floor:\n"})), False)
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
        gate_source = gate_text(ROOT)
    except OSError as error:
        expect("the gate is readable", str(error), "readable")
        gate_source = ""
    expect("the gate defines the one threshold exactly once",
           len(re.findall(r"^LINE_FLOOR\s*=", gate_source, re.MULTILINE)), 1)
    expect("the gate states the number the rule states", EXPECTED_FLOOR in gate_source, True)
    expect("the gate holds the one comparison once",
           len(re.findall(re.escape(EXPECTED_COMPARISON), gate_source)), 1)

    # A git-ignored cache directory holds no rule, so an unreadable file inside
    # one must not be scanned and must not fail the check.
    with tempfile.TemporaryDirectory() as tmp:
        tree = Path(tmp)
        (tree / "benchmarks" / ".cache").mkdir(parents=True)
        (tree / "benchmarks" / ".cache" / "blob.json").write_bytes(b"\x8a\x00")
        (tree / "Makefile").write_text("all:\n")
        expect(
            "a git-ignored cache file is not scanned",
            [path.relative_to(tree).as_posix() for path in candidate_paths(tree)],
            ["Makefile"],
        )

    # The real tree, which is the assertion that matters.
    found, unread = scan()
    expect("no file this check needs is unreadable", unread, [])
    expect("the real tree holds one rule", [(f.rule, f.path, f.line) for f in found], [])

    print(f"self-test: {'all passed' if failures == 0 else f'{failures} failed'}")
    return 0 if failures == 0 else 1
