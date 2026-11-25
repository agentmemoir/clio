#!/usr/bin/env python3
"""Checks that no workflow `run:` body holds a `${{ ... }}` expression.

The workflow engine expands a `${{ ... }}` before the shell sees the line, so the script the runner executes is not the script a reader finds in the file, and a value that comes from a pull request, an issue title, or another context lands inside the command text.
Passing the value through `env:` and reading it as a shell variable keeps the script text fixed and the value data, which is why this check refuses the expression and not the value.

This check reads the workflow lines and knows one key: `run:`.
An expression under `env:`, `with:`, `if:`, `name:`, `uses:`, `run-name:`, or any other key is not a finding, because the engine expands it into a field rather than into a command.
That is the whole rule, and it is why the check parses no YAML: a full parse would need a library, and the question here is which lines a `run:` key owns, not what the document means.
The ownership rule is the one a reader would use: a block scalar, or a plain value written on the lines below its key, owns every following line indented deeper than the column where its `run:` key starts, including the blank lines inside it, and it ends at the first non-blank line that is not deeper.

What it does not do: follow an anchor, an alias, or a merge key. A `run:` reached through one of those is not seen by a line reader; a `run:` key is seen whether it starts its line or follows a list dash on the same line (`- run: ...`).

Usage:
  python3 scripts/workflow_expression_check.py
  python3 scripts/workflow_expression_check.py --self-test

Exit codes:
  0  no `run:` body holds an expression
  1  at least one finding, each naming the file and line
  2  usage error
  3  a path this check had to read could not be read, so the clean parts of the run do not cover it
"""

from __future__ import annotations

import re
import sys
import tempfile
from pathlib import Path

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

USAGE = "usage: workflow_expression_check.py | --self-test"

WORKFLOW_DIR = ".github/workflows"

#: A `run:` mapping key at any indentation, with an inline value or none, and an
#: optional list dash on the same line. Group 1 is the leading spaces and group 2
#: the dash with its trailing spaces, so their lengths give the column of `run`.
RUN_KEY = re.compile(r"^([ ]*)(-[ ]+)?run:(?:[ ]+(.*))?$")

#: A block scalar header, with an indentation digit and a chomping sign in either
#: order, after a trailing comment is dropped.
BLOCK_HEADER = re.compile(r"^[|>](?:[1-9][+-]?|[+-][1-9]?)?$")

EXPRESSION = "${{"

MESSAGE = (
    "`${{ ... }}` is expanded before the shell runs this line, so the command text "
    "is not the text in the file. Pass the value through `env:` and read it as a "
    "shell variable"
)


def _inline(value: str | None) -> str:
    """The text a `run:` key holds on its own line, without one trailing comment."""
    return (value or "").split(" #", 1)[0].strip()


def run_bodies(text: str) -> list[tuple[int, str]]:
    """Every line a `run:` key holds, as (line number, text)."""
    found: list[tuple[int, str]] = []
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        number = index + 1
        match = RUN_KEY.match(lines[index])
        index += 1
        if not match:
            continue
        # The body boundary is measured from the `run` column, not from the dash.
        key_column = len(match.group(1)) + len(match.group(2) or "")
        value = _inline(match.group(3))
        if value and not BLOCK_HEADER.match(value):
            found.append((number, value))
            continue
        # A block header and an empty key both own the deeper lines below them.
        while index < len(lines):
            body = lines[index]
            if body.strip():
                if len(body) - len(body.lstrip(" ")) <= key_column:
                    break
                found.append((index + 1, body))
            index += 1
    return found


class Finding:
    def __init__(self, path: str, line: int, message: str) -> None:
        self.path = path
        self.line = line
        self.message = message

    def __str__(self) -> str:
        where = f"{self.path}:{self.line}" if self.line else self.path
        return f"{where}: workflow: {self.message}"


class Surface:
    def __init__(self) -> None:
        self.findings: list[Finding] = []
        self.unreadable: list[str] = []
        self.scanned: list[str] = []

    def exit_code(self) -> int:
        if self.unreadable:
            return EXIT_UNREADABLE
        return EXIT_FINDINGS if self.findings else EXIT_OK

    def read(self, root: Path, relative: str) -> str | None:
        self.scanned.append(relative)
        try:
            return (root / relative).read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            self.unreadable.append(f"{relative}: {error.strerror or error}")
            return None


def scan(root: Path) -> Surface:
    surface = Surface()

    workflow_dir = root / WORKFLOW_DIR
    if not workflow_dir.is_dir():
        surface.unreadable.append(f"{WORKFLOW_DIR}: no workflow directory")
        return surface

    try:
        workflows = sorted(workflow_dir.glob("*.yml")) + sorted(workflow_dir.glob("*.yaml"))
    except OSError as error:
        surface.unreadable.append(f"{WORKFLOW_DIR}: {error.strerror or error}")
        return surface

    for workflow in workflows:
        relative = f"{WORKFLOW_DIR}/{workflow.name}"
        text = surface.read(root, relative)
        if text is None:
            continue
        for number, body in run_bodies(text):
            if EXPRESSION in body:
                surface.findings.append(Finding(relative, number, MESSAGE))
    return surface


def report(root: Path) -> int:
    surface = scan(root)
    for path in surface.unreadable:
        print(f"workflow-expression: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"workflow-expression: FINDING {finding}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        print(
            f"workflow-expression: {len(surface.unreadable)} path(s) this check had to read could "
            "not be read, so the rest of this run does not cover them",
            file=sys.stderr,
        )
        return code
    if code == EXIT_FINDINGS:
        print(f"workflow-expression: {len(surface.findings)} finding(s)")
        return code
    print(
        f"workflow-expression: no run body holds an expression ({len(surface.scanned)} file(s) read)"
    )
    return code


CLEAN_WORKFLOW = """on: [push]
jobs:
  check:
    steps:
      - uses: actions/checkout@v4
      - name: Clippy
        run: cargo clippy --workspace --all-targets --locked -- -D warnings
"""


def _fixture(base: Path, label: str, changes: dict[str, str | None] | None = None) -> Path:
    files = {
        ".github/workflows/ci.yml": CLEAN_WORKFLOW,
    }
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


def _cases(base: Path) -> list[tuple[str, int, Path]]:
    wf = ".github/workflows/ci.yml"
    fields_only = """on: [push]
jobs:
  check:
    env:
      SHA: ${{ github.sha }}
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.sha }}
      - name: Clippy
        if: ${{ github.event_name == 'push' }}
        run: cargo clippy --workspace --all-targets --locked -- -D warnings
"""
    inline = """on: [push]
jobs:
  check:
    steps:
      - name: Echo
        run: echo ${{ github.sha }}
"""
    block = """on: [push]
jobs:
  check:
    steps:
      - name: Echo
        run: |
          echo "sha: ${{ github.sha }}"
          echo done
"""
    dash_inline = """on: [push]
jobs:
  check:
    steps:
      - run: echo ${{ github.sha }}
"""
    dash_block = """on: [push]
jobs:
  check:
    steps:
      - run: |
          echo "sha: ${{ github.sha }}"
          echo done
"""
    dash_bare = """on: [push]
jobs:
  check:
    steps:
      - run:
          echo "sha: ${{ github.sha }}"
"""
    block_digit = """on: [push]
jobs:
  check:
    steps:
      - name: Echo
        run: |2-
          echo "sha: ${{ github.sha }}"
          echo done
"""
    dash_sibling = """on: [push]
jobs:
  check:
    steps:
      - run: |
          echo done
        env:
          SHA: ${{ github.sha }}
"""
    folded = """on: [push]
jobs:
  check:
    steps:
      - name: Echo
        run: >
          echo "sha: ${{ github.sha }}"
          echo done
"""
    sibling = """on: [push]
jobs:
  check:
    steps:
      - name: Echo
        run: |
          echo done
        env:
          SHA: ${{ github.sha }}
"""
    no_run = """on: [push]
jobs:
  check:
    steps:
      - uses: actions/checkout@v4
"""
    return [
        ("expressions only in env, with and if pass", EXIT_OK, _fixture(base, "fields", {wf: fields_only})),
        ("an inline run body holding an expression is a finding", EXIT_FINDINGS, _fixture(base, "inline", {wf: inline})),
        ("a literal block holding an expression is a finding", EXIT_FINDINGS, _fixture(base, "block", {wf: block})),
        (
            "an inline run body after a list dash holding an expression is a finding",
            EXIT_FINDINGS,
            _fixture(base, "dash-inline", {wf: dash_inline}),
        ),
        (
            "a literal block after a list dash holding an expression is a finding",
            EXIT_FINDINGS,
            _fixture(base, "dash-block", {wf: dash_block}),
        ),
        (
            "a value on the lines below a bare list-dash run key is a finding",
            EXIT_FINDINGS,
            _fixture(base, "dash-bare", {wf: dash_bare}),
        ),
        (
            "a block header with an indentation digit holding an expression is a finding",
            EXIT_FINDINGS,
            _fixture(base, "block-digit", {wf: block_digit}),
        ),
        (
            "a list-dash block ends at a sibling key at the run column",
            EXIT_OK,
            _fixture(base, "dash-sibling", {wf: dash_sibling}),
        ),
        ("a folded block holding an expression is a finding", EXIT_FINDINGS, _fixture(base, "folded", {wf: folded})),
        (
            "a block body ends at a sibling key holding an expression",
            EXIT_OK,
            _fixture(base, "sibling", {wf: sibling}),
        ),
        ("a workflow with no run key passes", EXIT_OK, _fixture(base, "no-run", {wf: no_run})),
        ("a missing workflow directory is not a pass", EXIT_UNREADABLE, _fixture(base, "no-workflows", {wf: None})),
        (
            "a comment above a run key is not a run body",
            EXIT_OK,
            _fixture(base, "comment", {wf: CLEAN_WORKFLOW + "      # echo ${{ github.sha }}\n"}),
        ),
    ]


def _self_test() -> int:
    failures = 0
    total = 0
    with tempfile.TemporaryDirectory(prefix="workflow-expression-") as scratch:
        base = Path(scratch)
        for name, want, root in _cases(base):
            total += 1
            got = scan(root).exit_code()
            ok = got == want
            print(f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want})")
            failures += 0 if ok else 1

        # The exit code alone cannot say whether the finding names the line the expression sits on, and a reader with a block scalar needs that line rather than the key's. Asserted here for every finding case, because they take different paths through the reader.
        for label, want_count in (
            ("inline", 1),
            ("block", 1),
            ("dash-inline", 1),
            ("dash-block", 1),
            ("dash-bare", 1),
            ("block-digit", 1),
        ):
            total += 1
            root = base / label
            surface = scan(root)
            lines = [finding.line for finding in surface.findings]
            want_lines = [
                number
                for number, raw in enumerate((root / ".github/workflows/ci.yml").read_text().splitlines(), start=1)
                if EXPRESSION in raw
            ]
            ok = len(lines) == want_count and lines == want_lines and bool(want_lines)
            print(
                f"self-test {'ok' if ok else 'FAIL'}: a finding in {label} names the line, not only the file "
                f"(reported {lines}, want {want_lines})"
            )
            failures += 0 if ok else 1
    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total} cases)")
    return EXIT_OK


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return EXIT_OK
    if argv:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    return report(Path("."))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
