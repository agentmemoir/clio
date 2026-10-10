"""Read just enough Rust source to find which argument keys a function reads.

This is text analysis, not a parser. It blanks comments, finds functions and their
bodies by matching brackets outside strings, and follows a call only when the
call passes the argument object along. It answers one question for
`schema_args_check.py`: which literal keys can a handler read from `args`?

What it cannot see is stated by `Reads.unresolved`: a call it could not follow, so
the caller can refuse to call that handler checked.
"""

from __future__ import annotations

import re
from typing import NamedTuple

_LITERAL = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'|//[^\n]*|/\*.*?\*/', re.S)
_BRACKETS = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'|[(){}\[\]]', re.S)
_FN = re.compile(r"\bfn\s+(\w+)\s*(?:<[^>{(]*>)?\s*\(")
_CLOSERS = {"(": ")", "{": "}", "[": "]"}


def blank_comments(source: str) -> str:
    """Comments replaced by spaces (newlines kept); strings and chars left whole."""

    def blank(found: re.Match) -> str:
        text = found.group(0)
        return text if text[0] in "\"'" else re.sub(r"[^\n]", " ", text)

    return _LITERAL.sub(blank, source)


def close_index(text: str, start: int) -> int:
    """Index of the bracket closing the one at `start`; -1 if it never closes."""
    depth = 0
    for found in _BRACKETS.finditer(text, start):
        token = found.group(0)
        if token in _CLOSERS:
            depth += 1
        elif token in ")}]":
            depth -= 1
            if depth == 0:
                return found.start()
    return -1


_SPLIT = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'|[(){}\[\]]|,', re.S)


def split_commas(text: str) -> list[str]:
    """`text` split on commas that sit outside brackets and strings."""
    parts, depth, last = [], 0, 0
    for found in _SPLIT.finditer(text):
        token = found.group(0)
        if token in _CLOSERS:
            depth += 1
        elif token in ")}]":
            depth -= 1
        elif token == "," and depth == 0:
            parts.append(text[last : found.start()])
            last = found.end()
    parts.append(text[last:])
    return [part.strip() for part in parts if part.strip()]


class Fn(NamedTuple):
    name: str
    params: list[tuple[str, str]]
    body: str
    file: str


def functions(source: str, file: str = "") -> list[Fn]:
    """Every `fn` in `source` that has a body, with its parameters."""
    text = blank_comments(source)
    found: list[Fn] = []
    for head in _FN.finditer(text):
        open_paren = head.end() - 1
        close = close_index(text, open_paren)
        if close < 0:
            continue
        brace = text.find("{", close)
        semi = text.find(";", close)
        if brace < 0 or (0 <= semi < brace):
            continue
        end = close_index(text, brace)
        if end < 0:
            continue
        params = []
        for part in split_commas(text[open_paren + 1 : close]):
            name, _, kind = part.partition(":")
            params.append((name.strip().removeprefix("mut ").strip(), kind.strip()))
        found.append(Fn(head.group(1), params, text[brace + 1 : end], file))
    return found


def value_params(fn: Fn) -> list[str]:
    """Names of the parameters that hold a JSON value (`&Value`)."""
    return [n for n, kind in fn.params if kind.startswith("&") and kind.endswith("Value")]


def is_reader(fn: Fn) -> bool:
    """A helper shaped `(.., args: &Value, key: &str, ..)`: it reads the key it is given."""
    names = [n for n, _ in fn.params]
    kinds = dict(fn.params)
    for first, second in zip(names, names[1:]):
        if kinds[first].endswith("Value") and kinds[first].startswith("&") and second == "key":
            return True
    return False


class Reads(NamedTuple):
    keys: set[str]
    unresolved: set[str]


def _strip_ref(arg: str) -> str:
    return arg.replace("&", "").replace("*", "").replace("mut ", "").strip()


def reads_of(
    fn: Fn,
    names: list[str],
    index: dict[str, list[Fn]],
    readers: set[str],
    seen: frozenset = frozenset(),
) -> Reads:
    """Literal keys `fn` reads from the objects named `names`, following calls that pass them on."""
    keys: set[str] = set()
    unresolved: set[str] = set()
    if (fn.name, tuple(names)) in seen:
        return Reads(keys, unresolved)
    seen = seen | {(fn.name, tuple(names))}
    alt = "|".join(re.escape(name) for name in names)
    body = fn.body
    keys.update(re.findall(rf'\b(?:{alt})\s*\.\s*get\s*\(\s*"([^"]+)"', body))
    keys.update(re.findall(rf'\b(?:{alt})\s*\[\s*"([^"]+)"\s*\]', body))
    for call in re.finditer(r"(\.\s*)?\b(\w+)\s*\(", body):
        callee, method = call.group(2), bool(call.group(1))
        end = close_index(body, call.end() - 1)
        if end < 0:
            continue
        args = split_commas(body[call.end() : end])
        positions = [i for i, arg in enumerate(args) if _strip_ref(arg) in names]
        if not positions:
            continue
        if callee in readers:
            at = positions[0]
            literal = re.fullmatch(r'"([^"]+)"', args[at + 1]) if at + 1 < len(args) else None
            if literal and _strip_ref(args[at]) in names:
                keys.add(literal.group(1))
            continue
        targets = index.get(callee)
        if not targets:
            if callee not in _PASSIVE:
                unresolved.add(callee)
            continue
        if len(targets) > 1:
            unresolved.add(f"{callee} (defined {len(targets)} times)")
            continue
        target = targets[0]
        offset = 1 if method and target.params and target.params[0][0].endswith("self") else 0
        inner = [
            target.params[i + offset][0]
            for i in positions
            if i + offset < len(target.params)
        ]
        if inner:
            sub = reads_of(target, inner, index, readers, seen)
            keys |= sub.keys
            unresolved |= sub.unresolved
    return Reads(keys, unresolved)


#: Calls that are handed the argument object but cannot read a key from it by name.
_PASSIVE = frozenset(
    {"Some", "Ok", "Err", "Box", "format", "json", "clone", "to_owned", "len", "is_object",
     "as_object", "is_null", "to_string", "println", "eprintln", "debug_assert", "assert",
     "matches", "dbg", "vec", "from", "into", "map", "and_then", "unwrap_or", "filter"}
)
