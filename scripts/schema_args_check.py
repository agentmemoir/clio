#!/usr/bin/env python3
"""Static check: every argument a tool handler reads must be declared in its schema.

The MCP server publishes each tool's input schema, and `arg_contract::check` refuses
any argument the schema does not declare before the handler runs. A handler that
reads an undeclared key therefore has a branch no client can reach: the feature is
written, tested in process, and silently unavailable. This check compares, for each
tool, the literal keys its handler reads from `args` with the properties its schema
declares, without building anything.

    python3 scripts/schema_args_check.py [--root PATH]

A tool whose handler cannot be found by reading the source is listed as unchecked.
Unchecked is never a pass: the report names each one and why.

A mismatch listed in `KNOWN` is allowed, and an entry that no longer matches anything
fails the run, so the table can only shrink. `--no-baseline` ignores it and lists
every mismatch.

Exit codes: 0 nothing new, 1 a new mismatch or a stale `KNOWN` entry, 3 the source
could not be read.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import NamedTuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

from schema_args_rust import (  # noqa: E402
    Fn,
    blank_comments,
    close_index,
    functions,
    is_reader,
    reads_of,
    value_params,
)

SRC = Path("crates") / "clio-mcp" / "src"

#: Arguments every tool accepts without declaring them (`arg_contract::RUNTIME_ARGUMENTS`).
RUNTIME_ARGS = frozenset({"bank", "actor", "timestamp"})

#: Files whose match arms route batch operations, not tool calls. They read `params`, which
#: is each operation's own object, and are checked against the batch tool's own schema.
BATCH_PREFIX = "batch"

ROUTED = "routed by name"

GUARD = "refused on purpose: the handler reads it only to reject it with a message"
SHARED = "read through a shared helper; not yet judged"

#: Reads of an undeclared argument that exist on `master` today. Remove an entry when the
#: schema declares the argument or the read goes; a stale entry fails the run.
KNOWN: dict[tuple[str, str], str] = {
    **{pair: GUARD for pair in [
        ("consolidate", "scope"), ("sync_push", "sync_key"), ("sync_pull", "sync_key"),
        ("sync_serve", "token"),
    ]},
    **{pair: SHARED for pair in [
        ("sync_ack_skip", "remote"), ("sync_ack_skip", "sync_key"), ("sync_ack_skip", "token"),
        ("sync_status", "remote"), ("sync_status", "token"),
    ]},
}

_ARM = re.compile(r'"(\w+)"((?:\s*\|\s*"\w+")*)\s*=>')
_TUPLE_KEY = re.compile(r'\(\s*"(\w+)"\s*,')
_TOOLDEF = re.compile(r"\bToolDef\s*\{")


class Report(NamedTuple):
    mismatches: dict[str, tuple[set[str], str]]
    unchecked: dict[str, str]
    checked: list[str]


def source_files(root: Path) -> list[Path]:
    directory = root / SRC
    if not directory.is_dir():
        raise FileNotFoundError(f"{directory} is not there")
    return sorted(p for p in directory.glob("*.rs") if "_tests" not in p.name and not p.name.endswith("_test.rs"))


def load(files: list[Path]) -> tuple[dict[str, list[Fn]], set[str]]:
    index: dict[str, list[Fn]] = {}
    for path in files:
        for fn in functions(path.read_text(encoding="utf-8"), path.name):
            index.setdefault(fn.name, []).append(fn)
    readers = {name for defs in index.values() for fn in defs if is_reader(fn) for name in [fn.name]}
    return index, readers


def tuple_keys(text: str, index: dict[str, list[Fn]], seen: set[str]) -> set[str]:
    """Property names in a props expression, following calls to helpers defined in the crate."""
    keys = set(_TUPLE_KEY.findall(text))
    for call in re.finditer(r"\b(\w+)\s*\(", text):
        name = call.group(1)
        if name in seen or name not in index or len(index[name]) != 1:
            continue
        seen.add(name)
        keys |= tuple_keys(index[name][0].body, index, seen)
    return keys


def declared_props(root: Path, index: dict[str, list[Fn]]) -> dict[str, set[str]]:
    """Tool name to the property names its schema declares, read from the `schema_*` sources."""
    declared: dict[str, set[str]] = {}
    factories: dict[str, set[str]] = {}
    texts = []
    for path in source_files(root):
        if path.name.startswith("schema_"):
            text = blank_comments(path.read_text(encoding="utf-8"))
            texts.append(text)
            for block in _TOOLDEF.finditer(text):
                end = close_index(text, block.end() - 1)
                body = text[block.end() : end]
                literal = re.search(r'\bname\s*:\s*"(\w+)"', body)
                variable_props = re.search(r"\bprops\s*(?:,|\n\s*$|:\s*\w+\s*,)", body)
                if literal and not variable_props:
                    declared.setdefault(literal.group(1), set()).update(tuple_keys(body, index, set()))
                    continue
                owner = _enclosing(text, block.start())
                if owner:
                    scope = tuple_keys(owner[1], index, set())
                    if literal:
                        declared.setdefault(literal.group(1), set()).update(scope)
                    else:
                        factories[owner[0]] = scope
    for text in texts:
        for name, scope in factories.items():
            for call in re.finditer(rf'\b{re.escape(name)}\s*\(\s*"(\w+)"', text):
                end = close_index(text, text.index("(", call.start()))
                extra = tuple_keys(text[call.end() : end], index, set())
                declared.setdefault(call.group(1), set()).update(scope | extra)
    return declared


def _enclosing(text: str, position: int) -> tuple[str, str] | None:
    """The innermost `fn` or `let x = |..|` before `position`: its name and body text."""
    best = None
    for head in re.finditer(r"\bfn\s+(\w+)|\blet\s+(\w+)\s*=\s*\|", text[:position]):
        best = head
    if best is None:
        return None
    brace = text.find("{", best.end())
    end = close_index(text, brace) if brace >= 0 else -1
    if end < position:
        return None
    return (best.group(1) or best.group(2)), text[brace + 1 : end]


def arm_text(text: str, start: int) -> str:
    """The right-hand side of the match arm whose `=>` ends at `start`."""
    begin = start
    while begin < len(text) and text[begin].isspace():
        begin += 1
    if begin < len(text) and text[begin] == "{":
        return text[begin : close_index(text, begin) + 1]
    depth, index = 0, begin
    while index < len(text):
        char = text[index]
        if char == '"':
            index = text.find('"', index + 1) if text.find('"', index + 1) >= 0 else len(text)
        elif char in "([{":
            depth += 1
        elif char in ")]}":
            if depth == 0:
                break
            depth -= 1
        elif char == "," and depth == 0:
            break
        index += 1
    return text[begin:index]


def arms(files: list[Path], index: dict[str, list[Fn]], readers: set[str]) -> dict[str, list[tuple[set[str], set[str], str]]]:
    """Tool name to (reads, unresolved calls, file) for each match arm that handles it."""
    found: dict[str, list[tuple[set[str], set[str], str]]] = {}
    for path in files:
        if path.name.startswith(BATCH_PREFIX) or path.name.startswith("schema_"):
            continue
        for fn in functions(path.read_text(encoding="utf-8"), path.name):
            names = value_params(fn)
            tool_var = [n for n, kind in fn.params if kind.endswith("str") and n in {"name", "tool"}]
            if not names:
                continue
            for arm in _ARM.finditer(fn.body):
                rhs = arm_text(fn.body, arm.end())
                tools = [arm.group(1)] + re.findall(r'"(\w+)"', arm.group(2))
                if not any(re.search(rf"\b{re.escape(n)}\b", rhs) for n in names):
                    continue
                if any(re.search(rf"\b{re.escape(v)}\b", rhs) for v in tool_var):
                    for tool in tools:
                        found.setdefault(tool, []).append((set(), {f"{ROUTED} through {path.name}"}, path.name))
                    continue
                pseudo = Fn(fn.name, fn.params, rhs, path.name)
                result = reads_of(pseudo, names, index, readers)
                for tool in tools:
                    found.setdefault(tool, []).append((result.keys, result.unresolved, path.name))
    return found


def registry_names(root: Path) -> set[str]:
    """Tool names in `clio-types`' bound tool lists; empty when the file is absent."""
    path = root / "crates" / "clio-types" / "src" / "mcp_tools.rs"
    if not path.is_file():
        return set()
    text = blank_comments(path.read_text(encoding="utf-8"))
    return set(re.findall(r'^\s*"([a-z][a-z_0-9]*)",\s*$', text, re.M))


def check(root: Path) -> Report:
    files = source_files(root)
    index, readers = load(files)
    declared = declared_props(root, index)
    routed = arms(files, index, readers)
    mismatches: dict[str, tuple[set[str], str]] = {}
    unchecked: dict[str, str] = {}
    checked: list[str] = []
    for tool in sorted(registry_names(root) - set(declared)):
        unchecked[tool] = "its schema was not found in the schema_* sources"
    for tool in sorted(declared):
        found = routed.get(tool)
        if not found:
            unchecked[tool] = "no match arm hands it the arguments"
            continue
        # An arm that passes the tool name on only forwards the call. The tool is
        # checked from the arm that handles it; with no such arm it is unchecked.
        handled = [arm for arm in found if not any(n.startswith(ROUTED) for n in arm[1])]
        if not handled:
            unchecked[tool] = sorted(found[0][1])[0]
            continue
        found = handled
        reads = set().union(*(keys for keys, _, _ in found))
        extra = reads - declared[tool] - RUNTIME_ARGS
        checked.append(tool)
        if extra:
            mismatches[tool] = (extra, found[0][2])
    return Report(mismatches, unchecked, checked)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Handler reads must be declared in the tool schema.")
    parser.add_argument("--root", default=str(Path(__file__).resolve().parent.parent), help="checkout to read")
    parser.add_argument("--no-baseline", action="store_true", help="list every mismatch, ignoring KNOWN")
    args = parser.parse_args(argv)
    try:
        report = check(Path(args.root))
    except (OSError, ValueError) as error:
        print(f"schema-args-check: {error}", file=sys.stderr)
        return 3
    found = {(tool, key): file for tool, (keys, file) in report.mismatches.items() for key in keys}
    known = {} if args.no_baseline else KNOWN
    new = {pair: file for pair, file in found.items() if pair not in known}
    stale = sorted(pair for pair in known if pair not in found)
    total = len(report.checked) + len(report.unchecked)
    print(
        f"schema-args-check: {total} tool(s): {len(report.checked)} checked, {len(report.unchecked)} "
        f"unchecked; {len(new)} new mismatch(es), {len(found) - len(new)} known, {len(stale)} stale"
    )
    for (tool, key), file in sorted(new.items()):
        print(f"  mismatch: {tool} reads `{key}` ({file}), which its schema does not declare")
    for tool, key in stale:
        print(f"  stale: KNOWN lists {tool} `{key}`, which is no longer a mismatch; remove it")
    for tool, reason in sorted(report.unchecked.items()):
        print(f"  unchecked: {tool}: {reason}")
    return 1 if new or stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
