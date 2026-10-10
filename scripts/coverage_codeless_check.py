#!/usr/bin/env python3
"""Every file the coverage gate skips for holding no executable code must hold none.

`scripts/coverage_codeless.json` records the files the coverage gate skips for
holding no executable code. The runner now decides that property from each file's
own source and keeps the record as a cross-check, so an entry that is wrong is
caught rather than a file silently leaving the gate.

So this checks the list against the tree. A file qualifies when every item in it
compiles to no instrumented line: module declarations, `use` statements, type
aliases, `const` or `static` values — whose contents live in the binary's data
rather than in a counted region — and bodyless declarations such as a trait
method signature or an `extern` import, which generate no code at all. A
function with a body is the disqualifier, whatever its visibility, because a
function body is exactly the thing a coverage rule exists to measure; a `fn`
that ends in `;` has no body and does not disqualify. A `macro_rules!`
definition disqualifies too, since it can expand to functions that a reader
cannot see in the file.

Usage:
  python3 scripts/coverage_codeless_check.py
  python3 scripts/coverage_codeless_check.py --self-test

Exit codes:
  0  every entry still holds no function body
  1  an entry holds a function body, or a recorded path is gone
  2  usage error
  3  the list could not be read (fail closed)
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CODELESS_PATH = Path(__file__).resolve().parent / "coverage_codeless.json"
CODELESS_KEY = "_no_executable_code"

#: The `fn` keyword as its own token, followed by a name. The prefixes Rust
#: allows in front of `fn` — visibility, `default`, `const`, `async`, `unsafe`,
#: `extern "C"` — need no separate pattern here: the scan only needs the `fn`
#: token to reach `_has_body`, which tells a signature from a body. Matching the
#: token rather than an indented, space-separated line also catches the forms a
#: prefix pattern missed, such as `pub(crate)fn thing(){}` and `fn\nthing(){}`.
FUNCTION = re.compile(r"(?<![\w])fn\s+\w+", re.MULTILINE)

#: A macro that can expand to functions. A `macro_rules!` body is ordinary Rust
#: tokens, so this check cannot see what it produces, and an entry naming one has
#: to be refused rather than trusted.
MACRO = re.compile(
    r"^[ \t]*(?:macro_rules!|#\[proc_macro|#\[macro_export\])", re.MULTILINE
)

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2
EXIT_ERROR = 3


def entries(doc: object) -> dict[str, str]:
    """The recorded paths and their reasons, refusing anything malformed.

    A missing key is an empty list rather than an error: a checkout with no such
    file has recorded nothing, which is the same state as recording no such file.
    A key that maps to something other than an object, or an entry that is not a
    path and a reason, is refused, because a list that cannot be read cannot be
    checked and this check exists precisely to read it.
    """
    if not isinstance(doc, dict):
        raise ValueError(f"{CODELESS_PATH} is not a JSON object")
    recorded = doc.get(CODELESS_KEY, {})
    if not isinstance(recorded, dict):
        raise ValueError(f"{CODELESS_PATH}: {CODELESS_KEY!r} must be an object")
    out: dict[str, str] = {}
    for path, reason in recorded.items():
        if path.startswith("_"):
            continue
        if not isinstance(path, str) or not isinstance(reason, str):
            raise ValueError(
                f"{CODELESS_PATH}: every {CODELESS_KEY!r} entry maps a path to a "
                f"reason string, and {path!r} does not"
            )
        out[path] = reason
    return out


def _strip_noise(source: str) -> str:
    """Blank comments, string literals and char literals, keeping offsets.

    The body scan must not trip on a `;` or `{` that lives in prose or data
    rather than in code, so those spans become spaces. Angle brackets are not
    tracked: a stray `{` from a const-generic argument counts as a body, which
    fails safe, because the file then stays judged rather than skipped.
    """
    out = list(source)
    i, n = 0, len(source)
    while i < n:
        ch = source[i]
        nxt = source[i + 1] if i + 1 < n else ""
        if ch == "/" and nxt == "/":
            while i < n and source[i] != "\n":
                out[i] = " "
                i += 1
        elif ch == "/" and nxt == "*":
            depth = 0
            while i < n:
                if source[i] == "/" and i + 1 < n and source[i + 1] == "*":
                    depth += 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if source[i] == "*" and i + 1 < n and source[i + 1] == "/":
                    depth -= 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    if depth == 0:
                        break
                    continue
                if source[i] != "\n":
                    out[i] = " "
                i += 1
        elif ch == '"':
            out[i] = " "
            i += 1
            while i < n and source[i] != '"':
                if source[i] == "\\" and i + 1 < n:
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if source[i] != "\n":
                    out[i] = " "
                i += 1
            if i < n:
                out[i] = " "
                i += 1
        elif ch == "'":
            # A char literal is quote, one escaped-or-plain char, quote. A
            # lifetime or a label does not close after one character, so it
            # is left alone rather than read as a literal.
            j = i + 1
            if j < n and source[j] == "\\":
                j += 2
            else:
                j += 1
            if j < n and source[j] == "'":
                for k in range(i, j + 1):
                    out[k] = " "
                i = j + 1
            else:
                i += 1
        else:
            i += 1
    return "".join(out)


def _has_body(scan: str, start: int) -> bool:
    """Whether the `fn` at `start` carries a body, scanning to `;` or `{`.

    Depth counts only `()` and `[]`: an array element type holds a `;`
    (`-> [u8; 4]`) and parameters may hold either brace. The first `;` at
    depth zero ends a declaration; the first `{` at depth zero opens a body.
    Neither found is read as a body, the safe direction: the file stays
    judged rather than skipped.
    """
    depth = 0
    for i in range(start, len(scan)):
        ch = scan[i]
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth = max(0, depth - 1)
        elif depth == 0 and ch == ";":
            return False
        elif depth == 0 and ch == "{":
            return True
    return True


def offenders(source: str) -> list[str]:
    """The lines in `source` that make it a file with code, in file order.

    A `fn` counts only when it has a body: a trait method or an `extern`
    import ends in `;`, compiles to nothing, and can never appear in a
    coverage report, which is the rule this list exists to record.
    """
    scan = _strip_noise(source)
    found = [
        m.group(0).strip()
        for m in FUNCTION.finditer(scan)
        if _has_body(scan, m.start())
    ]
    found += [m.group(0).strip() for m in MACRO.finditer(scan)]
    return found


def check(paths: dict[str, str], read: object = Path.read_text) -> list[str]:
    """One finding per recorded path that no longer belongs in the list.

    `read` is a parameter so a case can supply a source without a file behind it.
    A recorded path that no longer exists is a finding too, in the safe
    direction: a stale entry is clutter, and a missing entry for a real file is
    the failure that would actually let code escape the gate, so a reader who
    sees this told to re-derive the list rather than to trust it.
    """
    findings: list[str] = []
    for path in sorted(paths):
        target = ROOT / path
        try:
            source = read(target, encoding="utf-8")
        except FileNotFoundError:
            findings.append(f"{path}: recorded here, but the file is gone")
            continue
        except OSError as error:
            findings.append(f"{path}: cannot be read: {error}")
            continue
        lines = offenders(source)
        if lines:
            shown = ", ".join(lines[:3])
            more = f" (+{len(lines) - 3} more)" if len(lines) > 3 else ""
            findings.append(f"{path}: holds code, so the gate must judge it: {shown}{more}")
    return findings


def self_test() -> int:
    """The rule, on inputs that cannot drift."""
    failures = 0

    def expect(name: str, got: object, want: object) -> None:
        nonlocal failures
        if got != want:
            failures += 1
            print(f"self-test FAIL: {name}: got {got!r}, want {want!r}")
        else:
            print(f"self-test ok: {name}")

    def reads(mapping: dict[str, str]):
        def read(target: Path, encoding: str = "utf-8") -> str:
            key = str(Path(target).relative_to(ROOT))
            if key not in mapping:
                raise FileNotFoundError(key)
            return mapping[key]

        return read

    # The real list, against the real tree. This is the assertion that matters:
    # every other case here proves the matcher, and this one proves the list.
    try:
        recorded = entries(json.loads(CODELESS_PATH.read_text(encoding="utf-8")))
    except (OSError, ValueError) as error:
        expect("the recorded list is readable", str(error), "readable")
        recorded = {}
    expect("an empty list yields no findings", check({}), [])
    expect("every recorded path still holds no code", check(recorded), [])

    # The matcher, on each shape that must disqualify a file.
    cases: list[tuple[str, str, bool]] = [
        ("a plain function", "pub fn thing() {}\n", True),
        ("a private function", "fn helper() {}\n", True),
        ("a const fn", "const fn make() -> u8 { 1 }\n", True),
        ("an async fn", "async fn go() {}\n", True),
        ("an unsafe fn", "unsafe fn raw() {}\n", True),
        ("a visibility with no space before fn", "pub(crate)fn thing() {}\n", True),
        ("a newline between fn and its name", "fn\nthing() {}\n", True),
        ("a visibility then a newline then fn", "pub(crate)\nfn thing() {}\n", True),
        ("a bodyless declaration with no space", 'extern "C"{fn c_fn();}\n', False),
        ("a type alias to a fn pointer", "type F = fn(u8) -> u8;\n", False),
        ("a method in an impl", "impl A {\n    pub fn call(&self) {}\n}\n", True),
        ("a foreign fn declaration", 'extern "C" {\n    fn c_fn();\n}\n', False),
        ("a trait method declaration", "trait Store: Send {\n    fn go(&self) -> u8;\n}\n", False),
        ("a multi-line trait method declaration", "trait Store {\n    fn revise(\n        &self,\n        bank: &str,\n    ) -> Result<(), Error>;\n}\n", False),
        ("a trait method with a default body", "trait Store {\n    fn go(&self) -> u8 {\n        1\n    }\n}\n", True),
        ("a declaration whose parameter holds an array", "trait Store {\n    fn go(&self, buf: [u8; 4]);\n}\n", False),
        ("a body after a comment holding a semicolon", "fn go() /* ; */ {\n    1\n}\n", True),
        ("a macro_rules definition", "macro_rules! go {\n    () => {};\n}\n", True),
        ("module declarations only", "mod a;\npub mod b;\n", False),
        ("use statements only", "use std::fmt;\npub use a::B;\n", False),
        ("type aliases only", "pub type R = ();\n", False),
        ("const and static values", "pub const N: usize = 2;\nstatic S: &str = \"\";\n", False),
        ("a mention of fn in a comment", "// fn never_defined() {}\n", False),
        ("a mention of fn in a doc comment", "/// calls fn thing() elsewhere\n", False),
        ("fn inside a string", 'const S: &str = "fn thing() {}";\n', False),
        ("an empty file", "", False),
    ]
    for name, source, must_flag in cases:
        got = bool(offenders(source))
        expect(f"{name} {'disqualifies' if must_flag else 'does not disqualify'}", got, must_flag)

    # The check function, end to end, without touching the tree.
    mapping = {
        "crates/a/src/clean.rs": "mod x;\n",
        "crates/a/src/dirty.rs": "fn thing() {}\n",
        "crates/a/src/gone.rs": None,  # type: ignore[dict-item]
    }

    def read_with_missing(target: Path, encoding: str = "utf-8") -> str:
        key = str(Path(target).relative_to(ROOT))
        value = mapping.get(key)
        if value is None:
            raise FileNotFoundError(key)
        return value

    findings = check(
        {"crates/a/src/clean.rs": "", "crates/a/src/dirty.rs": "", "crates/a/src/gone.rs": ""},
        read_with_missing,
    )
    expect("only the dirty and the missing file are findings", len(findings), 2)
    expect("the finding names the file with code", "dirty.rs" in findings[0], True)
    expect("the clean file produces no finding", any("clean.rs" in f for f in findings), False)
    expect("a recorded path that is gone is named too", "gone.rs" in findings[1], True)

    # The reader, against a list whose every path is fine.
    expect("a clean list produces no finding at all", check(
        {"crates/a/src/one.rs": ""}, reads({"crates/a/src/one.rs": "mod x;\npub type R = ();\n"})
    ), [])

    # The reader refuses a list it cannot read, rather than checking part of it.
    expect("a non-object document is refused", isinstance(
        _raises(ValueError, entries, [1, 2]), bool), True)
    expect("a key that is not an object is refused", _raises(
        ValueError, entries, {CODELESS_KEY: []}), True)
    expect("an entry that is not a reason string is refused", _raises(
        ValueError, entries, {CODELESS_KEY: {"a.rs": 7}}), True)
    expect("a note key beside the list is skipped", entries(
        {CODELESS_KEY: {"_why": "a note", "a.rs": "a reason"}}), {"a.rs": "a reason"})
    expect("a document with no such key is empty", entries({}), {})

    # An unreadable list fails closed in main(), not here.
    expect("the list path is the one the gate reads", CODELESS_PATH.name, "coverage_codeless.json")
    expect("the key is the one the gate reads", CODELESS_KEY, "_no_executable_code")

    with tempfile.TemporaryDirectory(prefix="codeless-check-") as scratch:
        path = Path(scratch) / "coverage_codeless.json"
        path.write_text(
            json.dumps({CODELESS_KEY: {"crates/a/src/gone.rs": "no longer there"}}),
            encoding="utf-8",
        )
        saved = globals()["CODELESS_PATH"]
        try:
            globals()["CODELESS_PATH"] = path
            code = main(["--quiet"])
        finally:
            globals()["CODELESS_PATH"] = saved
        expect("a list naming a file that is gone fails", code, EXIT_FAIL)

    print(f"self-test: {'all passed' if failures == 0 else f'{failures} failed'}")
    return EXIT_OK if failures == 0 else 1


def _raises(error: type, fn: object, *args: object) -> bool:
    """Whether `fn(*args)` raises `error`."""
    try:
        fn(*args)  # type: ignore[operator]
    except error:
        return True
    return False


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Refuse a recorded no-code file that holds code."
    )
    parser.add_argument("--self-test", action="store_true", help="run this script's own tests")
    parser.add_argument("--quiet", action="store_true", help="print nothing when the list is clean")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    try:
        recorded = entries(json.loads(CODELESS_PATH.read_text(encoding="utf-8")))
    except (OSError, ValueError) as error:
        print(f"codeless-check: cannot read the list: {error}", file=sys.stderr)
        return EXIT_ERROR
    findings = check(recorded)
    for finding in findings:
        print(f"codeless-check: FINDING {finding}")
    if findings:
        print(
            f"codeless-check: {len(findings)} recorded entr(ies) no longer hold no "
            "code, so the gate is skipping files it must judge",
            file=sys.stderr,
        )
        return EXIT_FAIL
    if not args.quiet:
        print(
            f"codeless-check: every one of the {len(recorded)} recorded files holds "
            "no function body, so every one of them is safe to skip"
        )
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))