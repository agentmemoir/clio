#!/usr/bin/env python3
"""Reading Rust source: comments, attributes, `mod` declarations, and nesting.

One scanner, shared by every part of the lookup. It is separate from the
question of which modules are tests because the two fail differently: this file
misreads a declaration, and a misread declaration is invisible, whereas the other
question is checked against the test runner's own list.

What this file decides, and how each part was established here rather than
assumed:

- **Comments are removed, string literals are kept.** A `mod` inside a doc comment
  is not a declaration, and matching one would attribute a file's tests to a
  module that does not exist. Literals must survive because `#[path = "..."]` is
  the only place a test file's real name appears.
- **`cfg(test)` is inherited.** A declaration inside `#[cfg(test)] mod inner { }`
  is gated too. Treating it as ungated loses test files whose gate sits on an
  enclosing block.
- **A gate written as `cfg(all(test, target_os = "linux"))` is a test gate with an
  operating system.** The operating system is read separately, and on another
  platform the module is reported as compiled only elsewhere. On a machine where
  it does not compile, naming it in a filter selects nothing.
- **A module may have both `foo.rs` and `foo/`.** Since Rust 2018 that module's
  children live in `foo/`. Four files in this workspace use the shape, and
  assuming one flat directory silently loses their submodules.
- **A `#[path]` resolves against the declaring file's directory**, not against
  that module's children directory. Using the children directory loses four test
  modules in one crate, measured.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import Iterator, NamedTuple

TEST_FUNCTION_MARKERS = ("#[test]", "#[tokio::test]", "#[rstest", "#[test_case]")
SUPPORT_SUFFIXES = ("_test_support.rs", "_fixtures.rs", "_seam.rs")

_ATTR = re.compile(r"#\s*\[([^\]]*)\]", re.S)
_TEST_GATE = re.compile(r"cfg\(\s*(?:all\(\s*)?test\s*[,)]")
_MOD = re.compile(r"(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z_0-9]*)\s*(\{|;)")
_PATH_ATTR = re.compile(r'path\s*=\s*"([^"]+)"')
_WORD = re.compile(r"[A-Za-z_0-9]")


class Declaration(NamedTuple):
    """One `mod` item, with the gating in force and the text of an inline body."""

    name: str
    path: str | None
    gated: bool
    inline: bool
    # True when the gate names an operating system, so the module exists on some
    # platforms and not others. Recorded separately because it is not a gate on
    # this platform: a module this platform does not compile cannot be selected,
    # and a filter naming it matches nothing.
    linux_only: bool = False
    body: str | None = None
    # Where the declaring file sits, so a `#[path]` can be resolved the way rustc
    # resolves it: against the declaring file's own directory.
    parent_dir: Path | None = None


class SourceError(Exception):
    """A source file could not be read."""


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        raise SourceError(f"cannot read {path}: {error}") from error


def strip_comments(source: str) -> str:
    """Remove comments, keep string literals.

    Rust strings have no nested-comment form, so a small scanner is enough and
    avoids depending on a parser. Raw strings are handled because a raw string can
    contain a `//` that must not start a comment.
    """
    out: list[str] = []
    i = 0
    n = len(source)
    while i < n:
        ch = source[i]
        if ch == "/" and i + 1 < n and source[i + 1] == "/":
            while i < n and source[i] != "\n":
                i += 1
        elif ch == "/" and i + 1 < n and source[i + 1] == "*":
            depth = 1
            i += 2
            while i < n and depth:
                if source.startswith("/*", i):
                    depth += 1
                    i += 2
                elif source.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
        elif ch == "r" and source.startswith('r#"', i):
            end = source.find('"#', i + 3)
            end = n if end == -1 else end + 2
            out.append(source[i:end])
            i = end
        elif ch == '"':
            out.append(ch)
            i += 1
            while i < n:
                if source[i] == "\\":
                    out.append(source[i : i + 2])
                    i += 2
                    continue
                out.append(source[i])
                if source[i] == '"':
                    i += 1
                    break
                i += 1
        else:
            out.append(ch)
            i += 1
    return "".join(out)


def attr_text(attrs: str) -> str:
    """Attribute text with `path` removed, quotes and `=` blanked, lowercased."""
    return _PATH_ATTR.sub("", attrs).replace('"', " ").replace("=", " ").lower()


def _is_gated(attrs: str) -> bool:
    """True for `cfg(test)` and for `cfg(all(test, ...))`.

    `cfg(all(test, target_os = "linux"))` is compiled only under test too. Its
    operating system is read separately, so on another platform the module is
    reported as compiled elsewhere instead of selected.
    """
    return _TEST_GATE.search(attr_text(attrs)) is not None


def _mod_here(text: str, index: int) -> bool:
    """True when the `mod` at `index` is the keyword and not part of a name."""
    if index and _WORD.match(text[index - 1]):
        return False
    after = index + 3
    return after >= len(text) or not _WORD.match(text[after])


def _body_after(text: str, start: int) -> str:
    """The text between the braces that open at `start`, brace-matched.

    Needed because an inline `mod tests { ... }` is a test module in its own
    right, and its body cannot be found any other way: it has no file, so nothing
    outside the text can be consulted to learn what is inside it.
    """
    depth = 1
    index = start
    length = len(text)
    while index < length and depth:
        ch = text[index]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return text[start:index]
        index += 1
    return text[start:]


def declarations(source: str, parent_dir: Path | None = None) -> Iterator[Declaration]:
    """Every `mod` item in `source`, with the gating in force.

    One linear pass. A stack holds the inline modules currently open, each with
    the gate in force at its depth, so a declaration's gating is whatever its
    enclosing blocks impose plus its own attributes.
    """
    text = strip_comments(source)
    open_gates: list[bool] = []
    pending: list[str] = []
    # The gate of an inline `mod x {` applies to its body, so the next `{` picks it
    # up. Without this the body is scanned as ungated and a test module nested
    # inside another test module is reported as not existing.
    next_brace_gate: bool | None = None
    index = 0
    length = len(text)
    while index < length:
        ch = text[index]
        if ch == "{":
            open_gates.append(bool(next_brace_gate))
            next_brace_gate = None
            pending = []
            index += 1
        elif ch == "}":
            if open_gates:
                open_gates.pop()
            pending = []
            index += 1
        elif ch == "#" and text.startswith("#[", index):
            end = text.find("]", index)
            if end == -1:
                break
            pending.append(text[index + 2 : end])
            index = end + 1
        elif text.startswith("mod", index) and _mod_here(text, index):
            match = _MOD.match(text, index)
            if match is None:
                index += 3
                continue
            attrs = " ".join(pending)
            path_match = _PATH_ATTR.search(attrs)
            inline = match.group(2) == "{"
            gated = any(open_gates) or _is_gated(attrs)
            yield Declaration(
                name=match.group(1),
                path=path_match.group(1) if path_match else None,
                gated=gated,
                linux_only="target_os" in attrs,
                inline=inline,
                body=_body_after(text, match.end()) if inline else None,
                parent_dir=parent_dir,
            )
            pending = []
            next_brace_gate = gated if inline else None
            index = match.end()
        else:
            index += 1


def children_dir(parent: Path) -> Path:
    """The directory a module's own submodules are read from.

    Not always the parent file's own directory. Since Rust 2018 a module may be
    `src/http.rs` while its submodules live in `src/http/`, and four files in this
    workspace use that shape. Assuming one flat directory silently loses those
    submodules.
    """
    if parent.name in ("lib.rs", "main.rs", "mod.rs"):
        return parent.parent
    nested = parent.parent / parent.stem
    return nested if nested.is_dir() else parent.parent


def resolve_child(child_dir: Path, decl: Declaration) -> Path | None:
    """The file a `mod` declaration points at, or None when it cannot exist."""
    if decl.path:
        # A `#[path]` value is resolved by rustc against the directory holding the
        # file that declares it, NOT against the module's children directory. The
        # two differ for a module with both `foo.rs` and `foo/`, which is how a
        # declaration inside `sqlite_persona_tests.rs` reaches a sibling file in
        # `src/` while that module's own children live in `src/sqlite_persona/`.
        bases = [b for b in (decl.parent_dir, child_dir) if b is not None]
        for base in bases:
            candidate = base / decl.path
            if candidate.exists():
                return candidate
        return None
    for option in (child_dir / f"{decl.name}.rs", child_dir / decl.name / "mod.rs"):
        if option.exists():
            return option
    return None


def holds_tests_text(source: str) -> bool:
    """True when `source` contains at least one test function."""
    return any(marker in source for marker in TEST_FUNCTION_MARKERS)


def holds_tests(path: Path) -> bool:
    """True when the file at `path` contains at least one test function."""
    return holds_tests_text(read_text(path))


def without_inline_bodies(source: str) -> str:
    """`source` with the text of every inline `mod x { ... }` body removed.

    Needed to decide whether a file's OWN module holds tests. A whole-file scan
    answers yes whenever a test exists anywhere in the file, including inside
    `#[cfg(test)] mod tests { ... }`, and then the file claims itself as a test
    module when no test reports that path. That produced 67 phantom test modules
    before this existed, every one harmless inside a wide filter and fatal as the
    only thing selected, because a filter matching nothing errors.
    """
    text = strip_comments(source)
    out: list[str] = []
    cursor = 0
    for decl in declarations(source):
        if not decl.inline:
            continue
        # `decl.body` is the text between the braces. Find where it starts so the
        # body is blanked rather than deleted, keeping what follows in place.
        marker = text.find("mod " + decl.name, cursor)
        if marker == -1:
            continue
        start = text.find("{", marker)
        if start == -1:
            continue
        out.append(text[cursor:start])
        cursor = start + 1 + len(decl.body or "")
    out.append(text[cursor:])
    return "".join(out)


PLATFORM_SKIPPED: list[str] = []
"""Modules left out because this platform does not compile them.

Filled as a side effect of the recursive walk and read once by the acceptance
check. A list threaded through every recursion would obscure the walk for a
value only the top of it consumes.

Reviewed 2026-10-10 per fix-inline-exceptions: keep. Empty is correct here;
without it a module compiled only elsewhere would report as unclaimed.
"""


def is_support_file(name: str) -> bool:
    """True for a shared-helpers file, which by the convention holds no tests.

    A closed list of exactly three suffixes, because the rule is what makes the
    owner rule safe to have an exemption: a shared helper has several users and so
    cannot have one owner, which is harmless only while it holds no test.
    Enforced by `accept_against_runner` in owned_accept.py; reviewed 2026-10-10.
    """
    return name.endswith(SUPPORT_SUFFIXES)


def test_modules_in(
    source: str,
    module_path: str,
    file_for_path: Path,
    platform_skipped: list[str] | None = None,
    gated: bool = False,
) -> list[str]:
    """Every test module reachable from `source`, as paths under `module_path`.

    `gated` is True when the whole source sits behind `cfg(test)` because its parent
    declared it so. A module it declares is then compiled only under test too, so a
    plain declaration inside it counts.

    Three shapes, and all three are real in this workspace:

    - the file holds `#[test]` functions at its own level, so its own module is a
      test module.
    - an out-of-line declaration gated `cfg(test)`, which is what finds
      `batch_tests_persona`. A plain `mod foo;` is not a test module, even when
      foo.rs holds tests: foo.rs owns them.
    - an inline `#[cfg(test)] mod tests { ... }` block, which is a test module with
      no file of its own. Measured as the largest single source of missed modules
      before inline blocks were handled.
    """
    found: set[str] = set()
    platform_skipped = platform_skipped if platform_skipped is not None else []
    # A file's own module counts only when a test sits at that file's own level.
    # A test inside `mod tests { ... }` belongs to the inline module instead, and
    # claiming both names the inline one twice.
    if holds_tests_text(without_inline_bodies(source)):
        found.add(module_path)
    for decl in declarations(source, file_for_path.parent):
        child_path = f"{module_path}::{decl.name}" if module_path else decl.name
        # Only a declaration behind `cfg(test)` is a test module: its own gate, or the
        # gate of the block or file it sits in. A plain `mod foo;` belongs to foo's own
        # file, even when foo.rs holds tests, so a crate root does not claim them.
        in_test = gated or decl.gated
        if decl.inline:
            body = decl.body or ""
            # Both conditions, not either. The gate alone is not enough: a file
            # here declares `#[cfg(test)] mod support { ... }` holding no test at
            # all, and claiming it produced 67 phantom test modules, measured.
            if in_test and holds_tests_text(without_inline_bodies(body)):
                found.add(child_path)
                found.update(
                    test_modules_in(body, child_path, file_for_path, platform_skipped, True)
                )
            continue
        child = resolve_child(children_dir(file_for_path), decl)
        if child is None or not in_test:
            continue
        child_source = read_text(child)
        # The child must HOLD tests. A gate on the declaration is not enough: 67
        # modules here are declared behind `cfg(test)` while containing no test at
        # all, one of them `main.rs` mounting `backup_cli_fixtures_tests.rs` as
        # `backup_cli_fixtures`, a file of fixtures only.
        if not holds_tests_text(child_source):
            continue
        if decl.linux_only and sys.platform != "linux":
            # Not compiled here, so no test in it can run or be measured. Reported
            # rather than selected: naming it in a filter is harmless only while
            # another branch matches something.
            platform_skipped.append(child_path)
            continue
        # The child's own module counts only for a test at the child's own level,
        # for the same reason this function's own check does.
        if holds_tests_text(without_inline_bodies(child_source)):
            found.add(child_path)
        found.update(
            test_modules_in(child_source, child_path, child, platform_skipped, True)
        )
    PLATFORM_SKIPPED.extend(platform_skipped)
    return sorted(found)
