#!/usr/bin/env python3
"""Tests for the per-description bound arm.

The packs here carry the bound object the CLI publishes, because the check reads
the limits from the pack it measures. The real exported fixture is used for the
scope proof: it still holds the pre-edit wording, so it shows that a frozen
location exists and that a long description outside the scope is never measured.

The counting tests need the pinned tokenizer and skip with a named reason when it
is absent; the scope and shape tests run either way.
"""

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import description_bounds
from offline_tokenizer import BudgetError, load

PACK = HERE.parents[1] / "crates" / "clio-mcp" / "tests" / "fixtures" / "schema_pack.json"

#: The object `clio mcp schema-export` publishes beside the catalog hash.
DEFINITION = {
    "max_unicode_scalar_values": 120,
    "max_tokens": 25,
    "unit": "one description, counted as Unicode scalar values and cl100k_base tokens",
    "tokenizer": "cl100k_base",
    "tokenizer_version": "0.14.0",
    "scope_rule": (
        "Each description in the frozen in-scope manifest is measured on its own, "
        "and a value at or below both limits passes."
    ),
}

SHORT = "Short sentence."
SCOPE_SIZE = 184


def require_tokenizer():
    """Return the pinned tokenizer, or skip with a named reason."""
    try:
        import tiktoken  # noqa: F401
    except ImportError as exc:
        raise unittest.SkipTest(f"tiktoken is not installed: {exc}") from None
    return load()


def real_pack():
    """Return the exported pack fixture as a mutable object."""
    return json.loads(PACK.read_text(encoding="utf-8"))


def locate(pack, location):
    """Return the tool object a scope location names, or None."""
    for tool in pack["tools"]:
        if tool["name"] == location["tool"]:
            return tool
    return None


def write_at(pack, location, text):
    """Set the description at ``location`` to ``text``."""
    tool = locate(pack, location)
    if location.get("property") is None:
        tool["description"] = text
        return
    tool["inputSchema"]["properties"][location["property"]]["description"] = text


def bounded_pack():
    """Return the exported fixture with the bound object and short in-scope text."""
    pack = real_pack()
    pack[description_bounds.DEFINITION_KEY] = dict(DEFINITION)
    for location in description_bounds.load_scope():
        write_at(pack, location, SHORT)
    return pack


def grown_pack(location, text):
    """Return a bounded pack whose one in-scope description is ``text``."""
    pack = bounded_pack()
    write_at(pack, location, text)
    return pack


def retrieve_property(prop):
    return {"tool": "retrieve", "property": prop}


class ScopeTests(unittest.TestCase):
    """The committed scope is frozen data and every entry must exist in a pack."""

    def test_committed_scope_holds_the_frozen_locations(self):
        scope = description_bounds.load_scope()
        self.assertEqual(len(scope), SCOPE_SIZE)
        self.assertEqual(len({(item["tool"], item["property"]) for item in scope}), SCOPE_SIZE)
        self.assertIn({"tool": "maintenance_status", "property": None}, scope)
        self.assertIn(retrieve_property("provenance_floor"), scope)
        self.assertIn(retrieve_property("min_scores"), scope)

    def test_the_real_pack_holds_every_scope_location(self):
        pack = real_pack()
        missing = [
            description_bounds.label_of(location)
            for location in description_bounds.load_scope()
            if description_bounds.description_at(pack, location) is None
        ]
        self.assertEqual(missing, [], f"frozen scope locations are absent: {missing}")

    def test_a_missing_location_fails_the_check(self):
        pack = bounded_pack()
        gone = [
            location
            for location in description_bounds.load_scope()
            if location["tool"] == "retrieve"
        ]
        pack["tools"] = [tool for tool in pack["tools"] if tool["name"] != "retrieve"]
        report = description_bounds.check(pack, require_tokenizer())
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(report["scope"]["missing"], len(gone))
        self.assertTrue(
            any("retrieve.provenance_floor" in failure for failure in report["failures"]),
            report["failures"],
        )


class BoundTests(unittest.TestCase):
    """A description passes at or below both limits and fails above either."""

    def test_within_bound_pack_passes(self):
        report = description_bounds.check(bounded_pack(), require_tokenizer())
        self.assertEqual(report["verdict"], "pass", report["failures"])
        self.assertEqual(report["scope"]["checked"], SCOPE_SIZE)
        self.assertEqual(report["scope"]["missing"], 0)
        self.assertEqual(report["breaches"], [])
        self.assertEqual(report["bound"]["max_unicode_scalar_values"], 120)
        self.assertEqual(report["bound"]["max_tokens"], 25)

    def test_a_long_description_outside_the_scope_never_fails(self):
        pack = bounded_pack()
        else_where = retrieve_property("entity_summary")
        long_text = "word " * 400
        write_at(pack, else_where, long_text)
        report = description_bounds.check(pack, require_tokenizer())
        self.assertGreater(len(long_text), 120)
        self.assertEqual(report["verdict"], "pass", report["failures"])

    def test_a_description_grown_past_120_characters_fails(self):
        location = retrieve_property("provenance_floor")
        report = description_bounds.check(
            grown_pack(location, "x" * 121), require_tokenizer()
        )
        self.assertEqual(report["verdict"], "fail", report)
        self.assertEqual(len(report["breaches"]), 1, report["breaches"])
        breach = report["breaches"][0]
        self.assertEqual(breach["location"], "retrieve.provenance_floor")
        self.assertEqual(breach["characters"], 121)
        message = report["failures"][0]
        for part in ("retrieve.provenance_floor", "121", "120", "25"):
            self.assertIn(part, message)

    def test_a_description_grown_past_25_tokens_fails(self):
        location = retrieve_property("include_exact")
        pack = grown_pack(location, " a" * 30)
        report = description_bounds.check(pack, require_tokenizer())
        self.assertEqual(report["verdict"], "fail", report)
        breach = report["breaches"][0]
        self.assertEqual(breach["location"], "retrieve.include_exact")
        self.assertEqual(breach["characters"], 60)
        self.assertEqual(breach["tokens"], 30)
        self.assertIn("30", report["failures"][0])

    def test_a_grown_tool_description_is_named_as_a_tool_description(self):
        location = {"tool": "maintenance_status", "property": None}
        report = description_bounds.check(
            grown_pack(location, "y" * 200), require_tokenizer()
        )
        self.assertEqual(report["verdict"], "fail", report)
        failure = report["failures"][0]
        self.assertIn("maintenance_status tool description", failure)

    def test_a_pack_without_the_published_bound_is_unusable(self):
        pack = bounded_pack()
        del pack[description_bounds.DEFINITION_KEY]
        with self.assertRaises(BudgetError) as caught:
            description_bounds.check(pack, require_tokenizer())
        self.assertIn(description_bounds.DEFINITION_KEY, str(caught.exception))


class CommandTests(unittest.TestCase):
    """The command exits 1 on a breach and prints one line per breach."""

    def run_main(self, argv):
        out = io.StringIO()
        err = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = description_bounds.main(argv)
        return code, out.getvalue(), err.getvalue()

    def test_a_grown_pack_exits_one_and_names_the_location(self):
        require_tokenizer()
        pack = grown_pack({"tool": "compose_context", "property": "fidelity"}, "z" * 130)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pack.json"
            path.write_text(json.dumps(pack), encoding="utf-8")
            code, text, err = self.run_main([str(path)])
        self.assertEqual(code, 1, err)
        self.assertIn("compose_context.fidelity", err)
        self.assertEqual(json.loads(text)["verdict"], "fail")

    def test_a_bounded_pack_exits_zero(self):
        require_tokenizer()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pack.json"
            path.write_text(json.dumps(bounded_pack()), encoding="utf-8")
            code, text, err = self.run_main([str(path)])
        self.assertEqual(code, 0, err)
        self.assertEqual(err, "")
        self.assertEqual(json.loads(text)["verdict"], "pass")

    def test_an_unusable_pack_exits_two(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pack.json"
            path.write_text("not json", encoding="utf-8")
            code, text, err = self.run_main([str(path)])
        self.assertEqual(code, 2)
        self.assertEqual(text, "")
        self.assertIn("mcp-description-bound:", err)


if __name__ == "__main__":
    unittest.main()
