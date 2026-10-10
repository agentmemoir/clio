#!/usr/bin/env python3
"""Checks that every workflow installs the toolchain the repository pins.

`rust-toolchain.toml` names one channel, and the workflows do not merely read it:
each one passes an explicit `toolchain:` to the install action, because that
action's own documentation says an explicit input can override the toolchain file
and a second project found the opposite. Naming it in both places removes the
question of which wins. It also creates one to keep: raise the pin without raising
the workflow and the runner compiles, lints and measures coverage with a different
compiler than the repository says it uses, and the workflow's own drift step only
finds out on the runner, after the push.

This check reads the pin and the workflows and refuses any version that disagrees,
so the disagreement is found before the push rather than on it.

It is a separate file from the lint-scope check on purpose. "Can a lint run select
fewer crates than it claims?" and "does the runner use the pinned compiler?" are
unrelated questions, and a file named for one of them should not be the place a
reader looks for the other.

What it does not do: resolve a matrix. A `toolchain: ${{ matrix.rust }}` is
reported, even in a matrix whose every value equals the pin, because proving that
needs a YAML parse of the matrix block and this check reads lines. The message
names what would settle it. That is a false positive a maintainer can see past,
rather than a rule that guesses.

Usage:
  python3 scripts/toolchain_pin_check.py
  python3 scripts/toolchain_pin_check.py --self-test

Exit codes:
  0  every toolchain a workflow installs equals the pin
  1  at least one finding, each naming the file and line
  2  usage error
  3  a file this check had to read could not be read, so the clean parts of the
     run do not cover it
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

USAGE = "usage: toolchain_pin_check.py | --self-test"

PIN_FILE = "rust-toolchain.toml"

WORKFLOW_DIR = ".github/workflows"

CHANNEL = re.compile(r'^\s*channel\s*=\s*"([^"]*)"')

TOOLCHAIN_INPUT = re.compile(r"^\s*toolchain:\s*(.+?)\s*$")

#: A value that is not a plain version, so it cannot be compared to the pin.
EXPRESSION = re.compile(r"\$\{|\$\(")


def _strip_comment(line: str) -> str:
    """Drop a whole-line comment and a trailing one outside quotes."""
    stripped = line.strip()
    if stripped.startswith("#"):
        return ""
    quote = None
    for index, char in enumerate(line):
        if quote is not None:
            if char == quote:
                quote = None
        elif char in "'\"":
            quote = char
        elif char == "#":
            return line[:index]
    return line


def pinned_version(text: str) -> str | None:
    """The channel the repository pins, or None when it names none."""
    for raw in text.splitlines():
        match = CHANNEL.match(raw)
        if match:
            return match.group(1)
    return None


def workflow_toolchains(text: str) -> list[tuple[int, str]]:
    """Every `toolchain:` value a workflow installs, as (line number, value)."""
    found: list[tuple[int, str]] = []
    for number, raw in enumerate(text.splitlines(), start=1):
        match = TOOLCHAIN_INPUT.match(_strip_comment(raw))
        if match:
            found.append((number, match.group(1).strip("\"'")))
    return found


class Finding:
    def __init__(self, path: str, line: int, message: str) -> None:
        self.path = path
        self.line = line
        self.message = message

    def __str__(self) -> str:
        where = f"{self.path}:{self.line}" if self.line else self.path
        return f"{where}: toolchain: {self.message}"


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
        except FileNotFoundError:
            self.findings.append(Finding(relative, 0, "a file this check must read is missing"))
            return None
        except OSError as error:
            self.unreadable.append(f"{relative}: {error.strerror or error}")
            return None


def scan(root: Path) -> Surface:
    surface = Surface()

    pin_file = surface.read(root, PIN_FILE)
    pinned = pinned_version(pin_file) if pin_file is not None else None
    if pin_file is not None and pinned is None:
        surface.findings.append(
            Finding(PIN_FILE, 0, "no channel is named, so there is no pin to compare against")
        )

    try:
        workflow_dir = root / WORKFLOW_DIR
        workflows = sorted(workflow_dir.glob("*.yml")) + sorted(workflow_dir.glob("*.yaml"))
    except OSError as error:
        surface.unreadable.append(f"{WORKFLOW_DIR}: {error.strerror or error}")
        workflows = []
    if not workflows:
        surface.unreadable.append(f"{WORKFLOW_DIR}: no workflow file could be listed")

    for workflow in workflows:
        relative = f"{WORKFLOW_DIR}/{workflow.name}"
        text = surface.read(root, relative)
        if text is None or pinned is None:
            continue
        for number, value in workflow_toolchains(text):
            if EXPRESSION.search(value):
                surface.findings.append(
                    Finding(
                        relative,
                        number,
                        f"`{value}` is an expression, so it cannot be compared with "
                        f"the pin {pinned}. This check reads lines and does not "
                        "resolve a matrix, so it cannot tell that every value the "
                        f"expression can take is {pinned}. Name {pinned} here, or "
                        "accept the finding until this check can read the matrix",
                    )
                )
            elif value != pinned:
                surface.findings.append(
                    Finding(
                        relative,
                        number,
                        f"installs {value} while the pin says {pinned}, so the runner "
                        "and the repository would build with different compilers",
                    )
                )
    return surface


def report(root: Path) -> int:
    surface = scan(root)
    for path in surface.unreadable:
        print(f"toolchain-pin: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"toolchain-pin: FINDING {finding}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        print(
            f"toolchain-pin: {len(surface.unreadable)} path(s) this check had to read could "
            "not be read, so the rest of this run does not cover them",
            file=sys.stderr,
        )
        return code
    if code == EXIT_FINDINGS:
        print(f"toolchain-pin: {len(surface.findings)} finding(s)")
        return code
    print(f"toolchain-pin: every workflow toolchain matches the pin ({len(surface.scanned)} file(s) read)")
    return code


CLEAN_WORKFLOW = """on: [push]
jobs:
  check:
    steps:
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
          components: rustfmt, clippy
      - name: Clippy
        run: cargo clippy --workspace --all-targets --locked -- -D warnings
"""

CLEAN_PIN = '[toolchain]\nchannel = "1.98.1"\ncomponents = ["rustfmt", "clippy"]\n'


def _fixture(base: Path, label: str, changes: dict[str, str | None] | None = None) -> Path:
    files = {
        PIN_FILE: CLEAN_PIN,
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
    # Two install steps, written out rather than produced by replacing a substring
    # of the one-step fixture: the substring surgery silently matched nothing on a
    # first attempt, and the case passed for the wrong reason.
    two_matching = """on: [push]
jobs:
  first:
    steps:
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
  second:
    steps:
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
      - name: Clippy
        run: cargo clippy --workspace --all-targets --locked -- -D warnings
"""
    one_drifting = two_matching.replace("  second:\n    steps:\n      - uses: dtolnay/rust-toolchain@master\n        with:\n          toolchain: 1.98.1",
                                        "  second:\n    steps:\n      - uses: dtolnay/rust-toolchain@master\n        with:\n          toolchain: 1.99.0")
    return [
        ("a matching toolchain passes", EXIT_OK, _fixture(base, "clean")),
        (
            "two matching toolchains in one file both pass",
            EXIT_OK,
            _fixture(base, "two", {wf: two_matching}),
        ),
        (
            "a toolchain that differs from the pin is a finding",
            EXIT_FINDINGS,
            _fixture(base, "drift", {wf: CLEAN_WORKFLOW.replace("1.98.1", "1.99.0")}),
        ),
        (
            "one of two toolchains differing is a finding",
            EXIT_FINDINGS,
            _fixture(base, "one-drift", {wf: one_drifting}),
        ),
        (
            "an expression cannot be compared and is a finding",
            EXIT_FINDINGS,
            _fixture(base, "expression", {wf: CLEAN_WORKFLOW.replace("toolchain: 1.98.1", "toolchain: ${{ matrix.rust }}")}),
        ),
        (
            "a commented toolchain is not installed",
            EXIT_OK,
            _fixture(base, "commented", {wf: CLEAN_WORKFLOW + "      # toolchain: 9.9.9\n"}),
        ),
        (
            "a pin file with no channel is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-channel", {PIN_FILE: "[toolchain]\ncomponents = [\"clippy\"]\n"}),
        ),
        ("a missing pin file is a finding", EXIT_FINDINGS, _fixture(base, "no-pin", {PIN_FILE: None})),
        (
            "a missing workflow directory is not a pass",
            EXIT_UNREADABLE,
            _fixture(base, "no-workflows", {wf: None}),
        ),
    ]


def _self_test() -> int:
    failures = 0
    total = 0
    with tempfile.TemporaryDirectory(prefix="toolchain-pin-") as scratch:
        base = Path(scratch)
        for name, want, root in _cases(base):
            total += 1
            got = scan(root).exit_code()
            ok = got == want
            print(f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want})")
            failures += 0 if ok else 1

        # A finding that names the file but not the line leaves a reader with two
        # install steps in one file and no idea which drifted. Asserted here rather
        # than as a case, because the exit code alone cannot tell them apart: the
        # case above already covers the code, and this covers what it reports.
        total += 1
        drifted = base / "one-drift"
        surface = scan(drifted)
        lines = [finding.line for finding in surface.findings]
        want_lines = [
            number
            for number, raw in enumerate((drifted / ".github/workflows/ci.yml").read_text().splitlines(), start=1)
            if "toolchain: 1.99.0" in raw
        ]
        ok = lines == want_lines and want_lines
        print(
            f"self-test {'ok' if ok else 'FAIL'}: a drift names the line, not only the file "
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
    return report(Path(__file__).resolve().parent.parent)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
