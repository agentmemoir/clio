#!/usr/bin/env python3
"""Check every in-scope MCP description against the pinned product rule.

The rule is pinned here: one description may hold at most 120 Unicode scalar
values and 25 cl100k_base tokens counted with tiktoken 0.14.0. The pack also
publishes a ``description_bound_definition``, and it must agree with this rule.
A pack that raises its own limit is a configuration error, not a pass, so the
check reads the limits from this file and never from the pack it measures.

The in-scope locations are frozen data in ``description_scope.json``, recorded
from the pack as it stood before the wording changed: each entry names one tool
and a JSON path inside that tool object, which reaches a property description,
a nested ``$defs`` property description, or the tool's own description. The
manifest also holds the frozen target names, and the pack must publish the same
names. A location that is absent from the pack fails instead of being skipped,
and a description that was never in scope is never measured.

Unicode scalar values are counted as the length of the decoded string, which
counts code points exactly. Tokens come from the repo-pinned offline tokenizer,
so two runs on one pack measure the same numbers.

An unusable input exits 2 with ``mcp-description-bound: <message>`` on stderr. A
breach exits 1, prints one line per breach on stderr, and still prints the report.
"""

import argparse
import json
import sys
from pathlib import Path

from measure import _parse_pack, _read_pack
from offline_tokenizer import BudgetError, load

HERE = Path(__file__).resolve().parent

SCOPE_FILE = HERE / "description_scope.json"
SCOPE_FORMAT = "clio_mcp_description_scope"
SCOPE_FORMAT_VERSION = 2

#: The pack key that publishes the bound this check enforces.
DEFINITION_KEY = "description_bound_definition"

#: The product rule. A pack may publish it, but it may never change it here.
PINNED = {
    "max_unicode_scalar_values": 120,
    "max_tokens": 25,
    "tokenizer": "cl100k_base",
    "tokenizer_version": "0.14.0",
}
UNIT = "one description, counted as Unicode scalar values and cl100k_base tokens"
SCOPE_RULE = (
    "Every JSON path listed in locations is resolved inside its tool object, and "
    "each one that holds a description is measured on its own; a value at or below "
    "both limits passes."
)

#: Every field the published definition must carry, checked by presence and type.
DEFINITION_TEXT_FIELDS = ("unit", "tokenizer", "tokenizer_version", "scope_rule")
DEFINITION_INT_FIELDS = ("max_unicode_scalar_values", "max_tokens")
DEFINITION_NAME_FIELDS = ("target_names", "tool_descriptions")


def _string_list(value, label) -> list:
    """Return ``value`` as a non-empty list of unique non-empty strings."""
    if not isinstance(value, list) or not value:
        raise BudgetError(f"{label} must be a non-empty list")
    seen = set()
    for item in value:
        if not isinstance(item, str) or not item:
            raise BudgetError(f"{label} holds an unusable name: {item!r}")
        if item in seen:
            raise BudgetError(f"{label} names {item!r} more than once")
        seen.add(item)
    return value


def load_manifest(path=SCOPE_FILE) -> dict:
    """Return the frozen scope manifest, refusing anything but that exact shape."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        raise BudgetError(f"cannot read the description scope: {exc}") from None
    try:
        document = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise BudgetError(f"description scope is not valid JSON: {exc}") from None
    if not isinstance(document, dict):
        raise BudgetError(
            f"description scope must be a JSON object, got {type(document).__name__}"
        )
    if document.get("format") != SCOPE_FORMAT:
        raise BudgetError(
            f"unsupported scope format: {document.get('format')!r}, "
            f"expected {SCOPE_FORMAT!r}"
        )
    if document.get("format_version") != SCOPE_FORMAT_VERSION:
        raise BudgetError(
            f"unsupported scope format version: {document.get('format_version')!r}"
        )
    for field in DEFINITION_NAME_FIELDS:
        _string_list(document.get(field), f"description scope {field}")
    locations = document.get("locations")
    if not isinstance(locations, list) or not locations:
        raise BudgetError("description scope has no locations")
    seen = set()
    for location in locations:
        if not isinstance(location, dict):
            raise BudgetError(f"scope location must be an object: {location!r}")
        tool = location.get("tool")
        path = location.get("path")
        if not isinstance(tool, str) or not tool:
            raise BudgetError(f"scope location has no tool name: {location!r}")
        if not isinstance(path, list) or not path:
            raise BudgetError(f"scope location has no path: {location!r}")
        for step in path:
            if not isinstance(step, str) or not step:
                raise BudgetError(f"scope location has an unusable path: {location!r}")
        key = (tool, tuple(path))
        if key in seen:
            raise BudgetError(f"duplicate scope location: {label_of(location)}")
        seen.add(key)
    return document


def load_scope(path=SCOPE_FILE) -> list:
    """Return only the frozen locations, for callers that do not need the names."""
    return load_manifest(path)["locations"]


def pinned_definition(manifest) -> dict:
    """Return the definition a pack must publish: the pinned rule plus its names."""
    return {
        "max_unicode_scalar_values": PINNED["max_unicode_scalar_values"],
        "max_tokens": PINNED["max_tokens"],
        "unit": UNIT,
        "tokenizer": PINNED["tokenizer"],
        "tokenizer_version": PINNED["tokenizer_version"],
        "scope_rule": SCOPE_RULE,
        "target_names": list(manifest["target_names"]),
        "tool_descriptions": list(manifest["tool_descriptions"]),
    }


def definition_of(pack, manifest) -> dict:
    """Return the bound the pack publishes, refusing a missing or malformed object."""
    definition = pack.get(DEFINITION_KEY)
    if not isinstance(definition, dict):
        raise BudgetError(f"pack has no {DEFINITION_KEY} object")
    for field in DEFINITION_INT_FIELDS:
        value = definition.get(field)
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            raise BudgetError(f"{DEFINITION_KEY}.{field} is not a positive integer")
    for field in DEFINITION_TEXT_FIELDS:
        value = definition.get(field)
        if not isinstance(value, str) or not value:
            raise BudgetError(f"{DEFINITION_KEY}.{field} is not a non-empty string")
    for field in DEFINITION_NAME_FIELDS:
        _string_list(definition.get(field), f"{DEFINITION_KEY}.{field}")
    _require_pinned(definition)
    _require_names(definition, manifest)
    return definition


def _require_pinned(definition) -> None:
    """Refuse a pack bound that disagrees with the product rule pinned here."""
    for field, expected in PINNED.items():
        actual = definition[field]
        if actual != expected:
            raise BudgetError(
                f"configuration error: {DEFINITION_KEY}.{field} is {actual!r}, but "
                f"the pinned product rule is {expected!r}; refusing to measure a "
                f"pack that declares its own bound"
            )


def _require_names(definition, manifest) -> None:
    """Refuse a pack whose published names differ from the frozen manifest."""
    for field in DEFINITION_NAME_FIELDS:
        published = definition[field]
        frozen = manifest[field]
        if published != frozen:
            raise BudgetError(
                f"configuration error: {DEFINITION_KEY}.{field} is {published!r}, "
                f"but the frozen scope manifest names {frozen!r}"
            )


def _bind_tokenizer(definition, tokenizer) -> None:
    """Refuse a pack bound this run cannot measure with the pinned tokenizer."""
    if definition["tokenizer"] != tokenizer.encoding:
        raise BudgetError(
            f"pack bound names tokenizer {definition['tokenizer']!r}, but this "
            f"check counts with {tokenizer.encoding!r}"
        )
    installed = tokenizer.provenance()["version"]
    if definition["tokenizer_version"] != installed:
        raise BudgetError(
            f"pack bound names tokenizer version {definition['tokenizer_version']!r}, "
            f"but tiktoken is {installed!r}"
        )


def label_of(location) -> str:
    """Return the human location of one scope entry, naming nested paths in full."""
    path = location["path"]
    if path == ["description"]:
        return f"{location['tool']} tool description"
    ignored = ("inputSchema", "properties", "description")
    steps = [step for step in path if step not in ignored]
    return ".".join([location["tool"], *steps])


def _tool_named(pack, name: str):
    """Return the one tool with ``name``, or None when the pack does not publish it."""
    matches = [
        tool
        for tool in pack.get("tools") or []
        if isinstance(tool, dict) and tool.get("name") == name
    ]
    if len(matches) > 1:
        raise BudgetError(f"pack publishes tool {name!r} more than once")
    return matches[0] if matches else None


def description_at(pack, location):
    """Resolve ``location``'s JSON path inside its tool; None when the pack lacks it."""
    tool = _tool_named(pack, location["tool"])
    if tool is None:
        return None
    node = tool
    for step in location["path"]:
        if not isinstance(node, dict) or step not in node:
            return None
        node = node[step]
    return node if isinstance(node, str) else None


def check(pack, tokenizer, manifest=None) -> dict:
    """Return the report for ``pack`` against the pinned rule and frozen scope."""
    manifest = load_manifest() if manifest is None else manifest
    definition = definition_of(pack, manifest)
    _bind_tokenizer(definition, tokenizer)
    max_chars = PINNED["max_unicode_scalar_values"]
    max_tokens = PINNED["max_tokens"]
    breaches = []
    missing = []
    checked = 0
    for location in manifest["locations"]:
        label = label_of(location)
        text = description_at(pack, location)
        if not isinstance(text, str) or not text:
            missing.append(
                f"description bound: {label} has no description, and it is in the "
                f"frozen scope"
            )
            continue
        checked += 1
        characters = len(text)
        tokens = tokenizer.count(text)
        if characters <= max_chars and tokens <= max_tokens:
            continue
        breaches.append(
            {
                "location": label,
                "characters": characters,
                "tokens": tokens,
                "message": (
                    f"description bound: {label} has {characters} Unicode scalar "
                    f"values and {tokens} {PINNED['tokenizer']} tokens; the limits "
                    f"are {max_chars} and {max_tokens}"
                ),
            }
        )
    failures = [breach["message"] for breach in breaches] + missing
    return {
        "bound": pinned_definition(manifest),
        "scope": {
            "locations": len(manifest["locations"]),
            "checked": checked,
            "missing": len(missing),
        },
        "breaches": breaches,
        "failures": failures,
        "verdict": "fail" if failures else "pass",
    }


def attach(report, pack, tokenizer) -> None:
    """Add this arm to a budget-check ``report``: its object, failures, and verdict."""
    result = check(pack, tokenizer)
    report["descriptions"] = result
    report["failures"].extend(result["failures"])
    if result["verdict"] != "pass":
        report["verdict"] = "fail"


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this check's stderr format."""

    def error(self, message):
        sys.stderr.write(f"mcp-description-bound: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Check ``pack`` and print the report; return the process exit code."""
    parser = _Parser(
        prog="description_bounds.py",
        description="Check in-scope MCP descriptions against the pinned product rule.",
    )
    parser.add_argument("pack", metavar="PATH", help="exported pack file to check")
    parser.add_argument(
        "--scope",
        metavar="PATH",
        default=None,
        help="scope manifest to use; defaults to the committed file",
    )
    args = parser.parse_args(argv)

    try:
        pack = _parse_pack(_read_pack(args.pack))
        manifest = load_manifest(args.scope or SCOPE_FILE)
        report = check(pack, load(), manifest)
    except BudgetError as exc:
        sys.stderr.write(f"mcp-description-bound: {exc}\n")
        return 2

    for failure in report["failures"]:
        sys.stderr.write(f"{failure}\n")
    sys.stdout.write(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    sys.stdout.flush()
    return 0 if report["verdict"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
