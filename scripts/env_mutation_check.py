#!/usr/bin/env python3
"""Refuses process-global environment mutation in Rust source.

`std::env::set_var` and `std::env::remove_var` change a value every thread in
the process shares, and Rust 2024 marks them `unsafe` for exactly that reason.
A test that calls one races every test running beside it: a suite that reads a
variable after a peer wrote it can pass or fail on scheduling alone. The rule
for tests is to pass configuration as a parameter or through a fixture.

This is the tripwire for that rule. It scans every `.rs` file under `crates/`
for a `set_var(` or `remove_var(` call and reports each one, because the
repository has no legitimate call site to preserve. The match is the bare
function name at a word boundary, so `std::env::set_var(`, an aliased
`e::set_var(`, and a bare `set_var(` after `use std::env::set_var;` all count.
The scan runs on source with string and character literals and comments blanked,
so a needle that only appears inside a literal or a comment is not a call and is
not reported. A production call that is genuinely needed (for example, setting a
variable for a child process is done on the child's `Command`, not on the
parent) gets its file named in ALLOWED, in the same change that adds it, with a
comment saying why.

It is a script, not a `#[test]`, because its subject is every crate while a
scoped run selects packages from the diff: a test inside `clio-store` would not
execute for a change to a crate that does not select it. It reads source and
runs no build.

Usage:
  python3 scripts/env_mutation_check.py
  python3 scripts/env_mutation_check.py --self-test

Exit codes:
  0  no offender, every scanned file was read
  1  at least one offender, each naming the file and line
  2  usage error
  3  a file or directory this check had to read could not be read
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

USAGE = "usage: env_mutation_check.py | --self-test"

CRATES = "crates"

#: A process-global mutator call, by the bare function name at a word boundary.
#: It catches `std::env::set_var(`, a path alias `e::set_var(`, and a bare
#: `set_var(` after a `use`, without matching `foo_set_var(`.
MUTATOR = re.compile(r"\b(set_var|remove_var)\s*\(")

#: A function definition named like a mutator is not a call to the std one.
FN_PREFIX = re.compile(r"\bfn\s+$")

GUIDANCE = "pass configuration as a parameter; do not mutate the process environment"

#: Repository-relative files allowed to mutate the process environment. A file
#: added here needs a comment giving the reason.
ALLOWED: frozenset[str] = frozenset()


class Finding:
    def __init__(self, path: str, line: int, needle: str) -> None:
        self.path = path
        self.line = line
        self.needle = needle

    def __str__(self) -> str:
        return f"{self.path}:{self.line}: {self.needle} ({GUIDANCE})"


class Surface:
    def __init__(self) -> None:
        self.findings: list[Finding] = []
        self.unreadable: list[str] = []
        self.scanned = 0

    def exit_code(self) -> int:
        if self.unreadable:
            return EXIT_UNREADABLE
        return EXIT_FINDINGS if self.findings else EXIT_OK


def rs_files(root: Path) -> list[Path]:
    base = root / CRATES
    if not base.is_dir():
        raise FileNotFoundError(f"{CRATES}: no source directory to scan")
    return sorted(path for path in base.rglob("*.rs") if path.is_file())


def _quoted_end(content: str, start: int, quote: str) -> int:
    """The offset just past the string opened by `quote` at `start`."""
    index = start + 1
    while index < len(content):
        char = content[index]
        if char == "\\":
            index += 2
            continue
        if char == quote:
            return index + 1
        index += 1
    return len(content)


def _raw_end(content: str, r_at: int) -> int | None:
    """The offset just past the raw string whose `r` is at `r_at`, or None."""
    index = r_at + 1
    hashes = 0
    while index < len(content) and content[index] == "#":
        hashes += 1
        index += 1
    if index >= len(content) or content[index] != '"':
        return None
    closer = '"' + "#" * hashes
    end = content.find(closer, index + 1)
    return len(content) if end == -1 else end + len(closer)


def _char_end(content: str, start: int) -> int | None:
    """The offset just past a character literal, or None for a lifetime."""
    limit = len(content)
    if start + 1 >= limit:
        return None
    if content[start + 1] == "\\":
        index = start + 2
        if content[index : index + 2] == "u{" and (brace := content.find("}", index + 2)) != -1:
            index = brace + 1
        else:
            index += 1
        return index + 1 if index < limit and content[index] == "'" else None
    if start + 2 < limit and content[start + 2] == "'":
        return start + 3
    return None


def _literal_end(content: str, start: int) -> int | None:
    """The offset just past a literal starting at `start`, or None.

    Handles normal and raw strings (with an optional `b`/`c` prefix), and
    character literals. A lifetime such as `'static` is not a literal.
    """
    before = content[start - 1] if start else ""
    prefixed_raw = content[start] in "bBcC" and content[start + 1 : start + 2] == "r"
    plain_raw = content[start] in "rR"
    if (prefixed_raw or plain_raw) and not (before.isalnum() or before == "_"):
        r_at = start + 1 if prefixed_raw else start
        return _raw_end(content, r_at)
    if content[start] == '"':
        return _quoted_end(content, start, '"')
    if content[start] == "'":
        return _char_end(content, start)
    return None


def _mask_code(content: str) -> str:
    """Blank string/char literal and comment text, preserving line breaks.

    Matching runs on the masked text so a needle that only appears inside a
    literal or a comment is not a call. Newlines are left in place so a reported
    line number still points at the original source.
    """
    out = list(content)
    limit = len(content)

    def blank(start: int, end: int) -> None:
        for index in range(start, end):
            if out[index] != "\n":
                out[index] = " "

    index = 0
    while index < limit:
        char = content[index]
        nxt = content[index + 1] if index + 1 < limit else ""
        if char == "/" and nxt == "/":
            end = content.find("\n", index)
            end = limit if end == -1 else end
            blank(index, end)
            index = end
        elif char == "/" and nxt == "*":
            depth = 1
            cursor = index + 2
            while cursor < limit and depth:
                if content[cursor : cursor + 2] == "/*":
                    depth += 1
                    cursor += 2
                elif content[cursor : cursor + 2] == "*/":
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            blank(index, cursor)
            index = cursor
        else:
            end = _literal_end(content, index)
            if end is None:
                index += 1
            else:
                blank(index, end)
                index = end
    return "".join(out)


def scan(root: Path, allowed: frozenset[str] | None = None) -> Surface:
    allowed = ALLOWED if allowed is None else allowed
    surface = Surface()
    try:
        files = rs_files(root)
    except FileNotFoundError as error:
        surface.unreadable.append(str(error))
        return surface
    except OSError as error:
        surface.unreadable.append(f"{CRATES}: {error.strerror or error}")
        return surface
    if not files:
        surface.unreadable.append(f"{CRATES}: no Rust source file was found to scan")
        return surface

    contents: dict[str, str] = {}
    for file in files:
        relative = file.relative_to(root).as_posix()
        try:
            contents[relative] = file.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            surface.unreadable.append(f"{relative}: {error.strerror or error}")
    surface.scanned = len(contents)

    for relative, content in contents.items():
        if relative in allowed:
            continue
        # The name has to appear; skip the masking pass for files that cannot
        # carry a call, which is nearly all of them.
        if "set_var" not in content and "remove_var" not in content:
            continue
        masked = _mask_code(content)
        for match in MUTATOR.finditer(masked):
            name_at = match.start(1)
            if FN_PREFIX.search(masked[:name_at]):
                continue
            line = masked.count("\n", 0, name_at) + 1
            surface.findings.append(Finding(relative, line, f"{match.group(1)}("))
    return surface


def report(root: Path) -> int:
    surface = scan(root)
    for path in surface.unreadable:
        print(f"env-mutation: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"env-mutation: FINDING {finding}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        print(
            f"env-mutation: {len(surface.unreadable)} path(s) this check had to read "
            "could not be read, so the rest of this run does not cover them",
            file=sys.stderr,
        )
        return code
    if code == EXIT_FINDINGS:
        print(f"env-mutation: {len(surface.findings)} finding(s)")
        return code
    print(f"env-mutation: no offender in {surface.scanned} source file(s)")
    return code


def _write(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _fixture(base: Path, label: str, files: dict[str, str]) -> Path:
    root = base / label
    for relative, text in files.items():
        _write(root, relative, text)
    return root


def _cases(base: Path) -> list[tuple[str, int, Path]]:
    plain = "crates/clio-store/src/leaf.rs"
    return [
        (
            "a clean file passes",
            EXIT_OK,
            _fixture(base, "clean", {plain: 'fn r() { let _ = std::env::var("X"); }\n'}),
        ),
        (
            "a set_var call is a finding",
            EXIT_FINDINGS,
            _fixture(base, "set", {plain: 'fn t() { std::env::set_var("X", "1"); }\n'}),
        ),
        (
            "a remove_var call is a finding",
            EXIT_FINDINGS,
            _fixture(base, "remove", {plain: 'fn t() { env::remove_var("X"); }\n'}),
        ),
        (
            "a line comment naming set_var passes",
            EXIT_OK,
            _fixture(base, "comment", {plain: "// never std::env::set_var here\nfn r() {}\n"}),
        ),
        (
            "a block comment naming set_var passes",
            EXIT_OK,
            _fixture(
                base,
                "block-comment",
                {plain: "/* set_var(\"X\", \"1\") lives in the docs */\nfn r() {}\n"},
            ),
        ),
        (
            "a string literal naming set_var passes",
            EXIT_OK,
            _fixture(
                base,
                "string",
                {plain: 'fn r() { let _ = "std::env::set_var(\\"X\\")"; }\n'},
            ),
        ),
        (
            "an aliased env call is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "alias",
                {plain: 'use std::env as e;\nfn t() { e::set_var("X", "1"); }\n'},
            ),
        ),
        (
            "a bare imported call is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "import",
                {plain: 'use std::env::set_var;\nfn t() { set_var("X", "1"); }\n'},
            ),
        ),
        (
            "a missing crates directory is not a pass",
            EXIT_UNREADABLE,
            _fixture(base, "no-crates", {"README.md": "no crates here\n"}),
        ),
        (
            "an empty crates directory is not a pass",
            EXIT_UNREADABLE,
            _fixture(base, "empty", {"crates/.keep": ""}),
        ),
    ]


def _self_test() -> int:
    failures = 0
    total = 0
    with tempfile.TemporaryDirectory(prefix="env-mutation-") as scratch:
        base = Path(scratch)
        for name, want, root in _cases(base):
            total += 1
            got = scan(root).exit_code()
            ok = got == want
            print(f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want})")
            failures += 0 if ok else 1

        # An allowlisted file is exempt; the file list is the only signal a
        # writer gets, so a genuine call site is named there in the same change.
        total += 1
        allowed_rel = "crates/clio-store/src/leaf.rs"
        exempt = _fixture(
            base,
            "allowed",
            {allowed_rel: "fn t() { std::env::set_var(\"X\", \"1\"); }\n"},
        )
        got = scan(exempt, frozenset({allowed_rel})).exit_code()
        ok = got == EXIT_OK
        print(f"self-test {'ok' if ok else 'FAIL'}: an allowlisted file passes (exit {got}, want {EXIT_OK})")
        failures += 0 if ok else 1

        # A finding that names the file but not the line leaves a reader with a
        # large file and no idea which call is the offender.
        total += 1
        planted = _fixture(
            base,
            "names-line",
            {"crates/clio-store/src/leaf.rs": "fn a() {}\n\nfn t() {\n    std::env::set_var(\"X\", \"1\");\n}\n"},
        )
        findings = scan(planted).findings
        want_line = 4
        ok = len(findings) == 1 and findings[0].line == want_line
        print(
            f"self-test {'ok' if ok else 'FAIL'}: a finding names the line "
            f"({[f.line for f in findings]}, want [{want_line}])"
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
