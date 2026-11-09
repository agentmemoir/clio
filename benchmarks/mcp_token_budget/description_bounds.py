#!/usr/bin/env python3
"""Check every in-scope MCP description against the bound its pack publishes.

The in-scope locations are frozen data in `description_scope.json`, recorded from
the pack as it stood before the wording changed: each entry names one tool and one
input-schema property, or a ``null`` property for that tool's own description. A
location that is absent from the pack fails instead of being skipped, and a
description that was never in scope is never measured.

The bound is read from the pack's ``description_bound_definition`` object, so the
two limits, the unit, the tokenizer, and the scope rule travel with the pack
instead of living here. A description passes at or below both limits and fails
above either one, and the failure line names the location and both numbers.

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
SCOPE_FORMAT_VERSION = 1

#: The pack key that publishes the bound this check enforces.
DEFINITION_KEY = "description_bound_definition"

#: Every field the published definition must carry, checked by presence and type.
DEFINITION_TEXT_FIELDS = ("unit", "tokenizer", "tokenizer_version", "scope_rule")
DEFINITION_INT_FIELDS = ("max_unicode_scalar_values", "max_tokens")


def load_scope(path=SCOPE_FILE) -> list:
    """Return the frozen in-scope locations, refusing anything but that exact shape."""
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
    locations = document.get("locations")
    if not isinstance(locations, list) or not locations:
        raise BudgetError("description scope has no locations")
    seen = set()
    for location in locations:
        if not isinstance(location, dict):
            raise BudgetError(f"scope location must be an object: {location!r}")
        tool = location.get("tool")
        prop = location.get("property")
        if not isinstance(tool, str) or not tool:
            raise BudgetError(f"scope location has no tool name: {location!r}")
        if prop is not None and (not isinstance(prop, str) or not prop):
            raise BudgetError(f"scope location has an unusable property: {location!r}")
        key = (tool, prop)
        if key in seen:
            raise BudgetError(f"duplicate scope location: {tool}.{prop}")
        seen.add(key)
    return locations


def definition_of(pack) -> dict:
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
    return definition


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
    """Return the human location of one scope entry: ``tool.property`` or tool description."""
    prop = location.get("property")
    if prop is None:
        return f"{location['tool']} tool description"
    return f"{location['tool']}.{prop}"


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
    """Return the description at ``location``, or None when the pack does not hold it."""
    tool = _tool_named(pack, location["tool"])
    if tool is None:
        return None
    prop = location.get("property")
    if prop is None:
        return tool.get("description")
    schema = tool.get("inputSchema")
    properties = schema.get("properties") if isinstance(schema, dict) else None
    spec = properties.get(prop) if isinstance(properties, dict) else None
    return spec.get("description") if isinstance(spec, dict) else None


def check(pack, tokenizer, scope=None) -> dict:
    """Return the report for ``pack`` against the published bound and frozen scope."""
    locations = load_scope() if scope is None else scope
    definition = definition_of(pack)
    _bind_tokenizer(definition, tokenizer)
    max_chars = definition["max_unicode_scalar_values"]
    max_tokens = definition["max_tokens"]
    breaches = []
    missing = []
    checked = 0
    for location in locations:
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
                    f"values and {tokens} {definition['tokenizer']} tokens; the "
                    f"limits are {max_chars} and {max_tokens}"
                ),
            }
        )
    failures = [breach["message"] for breach in breaches] + missing
    return {
        "bound": {
            "max_unicode_scalar_values": max_chars,
            "max_tokens": max_tokens,
            "unit": definition["unit"],
            "tokenizer": definition["tokenizer"],
            "tokenizer_version": definition["tokenizer_version"],
            "scope_rule": definition["scope_rule"],
        },
        "scope": {
            "locations": len(locations),
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
        description="Check in-scope MCP descriptions against the pack's published bound.",
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
        scope = load_scope(args.scope or SCOPE_FILE)
        report = check(pack, load(), scope)
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
