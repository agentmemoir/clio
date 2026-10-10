#!/usr/bin/env python3
"""Decide whether Rust source holds executable code.

The scoped coverage gate judges every changed Rust file by its line percentage,
and llvm-cov can only report a file that has an instrumented line. A file whose
items are all module declarations, `use` statements, type aliases, `const` or
`static` values, and bodyless declarations compiles to no counted line, so no
report can contain it and no coverage number can apply to it. `offenders(source)`
answers the one question behind that separation: does the file hold a function
body or a macro that can expand to one? An empty result means no counted line.

A `fn` that ends in `;` has no body, compiles to nothing, and does not
disqualify. A `macro_rules!` definition does disqualify, because its body is
ordinary tokens that can expand to a function a reader cannot see in the file.
"""

from __future__ import annotations

import re

#: The `fn` keyword as its own token, followed by a name. The prefixes Rust
#: allows in front of `fn` — visibility, `default`, `const`, `async`, `unsafe`,
#: `extern "C"` — need no separate pattern here: the scan only needs the `fn`
#: token to reach `_has_body`, which tells a signature from a body. Matching the
#: token rather than an indented, space-separated line also catches the forms a
#: prefix pattern missed, such as `pub(crate)fn thing(){}` and `fn\nthing(){}`.
FUNCTION = re.compile(r"(?<![\w])fn\s+\w+", re.MULTILINE)

#: A macro that can expand to functions. A `macro_rules!` body is ordinary Rust
#: tokens, so this check cannot see what it produces, and a file naming one has
#: to be refused rather than trusted.
MACRO = re.compile(
    r"^[ \t]*(?:macro_rules!|#\[proc_macro|#\[macro_export\])", re.MULTILINE
)


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
    coverage report, which is the rule this answers.
    """
    scan = _strip_noise(source)
    found = [
        m.group(0).strip()
        for m in FUNCTION.finditer(scan)
        if _has_body(scan, m.start())
    ]
    found += [m.group(0).strip() for m in MACRO.finditer(scan)]
    return found
