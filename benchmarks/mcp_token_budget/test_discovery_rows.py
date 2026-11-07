#!/usr/bin/env python3
"""Tests for the discovery rows recorder and the guards inside it.

Every check here is pure: the tests build a small tools array, canonicalize it
with the recorder's own helpers, and confirm that a row, the derived cost, the
tool-count guard, and the build-identity guard answer as the recorder promises.
No server is started, no release binary is needed, and no network is used, so
the suite stays fast and runs before a binary exists.
"""

import hashlib
import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import discovery_rows
import offline_tokenizer

TOOLS = [
    {
        "name": "remember",
        "description": "Store a fact.",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    },
    {
        "name": "recall",
        "description": "Find a fact.",
        "inputSchema": {"type": "object", "properties": {}},
    },
]
OFF_TOOLS = [{"name": f"tool_{index}"} for index in range(6)]
ON_TOOLS = OFF_TOOLS + [{"name": f"discovery_{index}"} for index in range(3)]


def require_tokenizer(test):
    """Return the offline tokenizer, or skip ``test`` naming what is missing."""
    try:
        return offline_tokenizer.load()
    except offline_tokenizer.BudgetError as exc:
        test.skipTest(
            "the token counts cannot be recomputed without the pinned tokenizer: "
            f"{exc}; install tiktoken and put its site-packages on PYTHONPATH"
        )


def make_rows(off_tools=OFF_TOOLS, on_tools=ON_TOOLS):
    """Return two handcrafted rows whose size fields are independent inputs."""
    def one(flag, tools, byte_step):
        return {
            "flag": flag,
            "label": "Measured",
            "tools": len(tools),
            "bytes": 100 + byte_step,
            "tokens": 20 + byte_step,
            "input_sha256": "0" * 64,
        }

    return [one("off", off_tools, 0), one("on", on_tools, 7)]


class RowTests(unittest.TestCase):
    def test_a_row_canonicalizes_the_tools_array_and_counts_its_bytes(self):
        tokenizer = require_tokenizer(self)
        measured = discovery_rows.row("off", TOOLS, tokenizer)
        canonical = json.dumps(
            TOOLS, ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
        self.assertEqual(measured["flag"], "off")
        self.assertEqual(measured["label"], "Measured")
        self.assertEqual(measured["tools"], len(TOOLS))
        self.assertEqual(measured["bytes"], len(canonical))
        self.assertEqual(measured["tokens"], tokenizer.count(canonical.decode("utf-8")))
        self.assertEqual(
            measured["input_sha256"], hashlib.sha256(canonical).hexdigest()
        )

    def test_canonicalization_sorts_keys_and_drops_whitespace(self):
        first = discovery_rows.canonical_bytes([{"b": 1, "a": 2}])
        second = discovery_rows.canonical_bytes([{"a": 2, "b": 1}])
        self.assertEqual(first, second)
        self.assertEqual(first, b'[{"a":2,"b":1}]')


class CostTests(unittest.TestCase):
    def test_cost_is_derived_as_on_minus_off(self):
        rows = make_rows()
        self.assertEqual(
            discovery_rows.cost(rows), {"bytes": 7, "tokens": 7}
        )

    def test_cost_rejects_a_missing_flag_row(self):
        with self.assertRaises(offline_tokenizer.BudgetError):
            discovery_rows.cost(make_rows()[:1])


class ToolCountGuardTests(unittest.TestCase):
    def test_minimal_off_count_must_be_six(self):
        rows = make_rows(
            off_tools=OFF_TOOLS[:5], on_tools=OFF_TOOLS[:5] + ON_TOOLS[-3:]
        )
        with self.assertRaises(offline_tokenizer.BudgetError) as caught:
            discovery_rows.check_tool_counts("minimal", rows)
        self.assertIn("expected 6", str(caught.exception))

    def test_on_count_must_be_off_plus_three(self):
        rows = make_rows(on_tools=OFF_TOOLS + ON_TOOLS[:2])
        with self.assertRaises(offline_tokenizer.BudgetError) as caught:
            discovery_rows.check_tool_counts("minimal", rows)
        self.assertIn("served 8", str(caught.exception))
        self.assertIn("expected 9", str(caught.exception))

    def test_the_expected_counts_pass(self):
        self.assertIsNone(discovery_rows.check_tool_counts("minimal", make_rows()))


class BuildIdentityGuardTests(unittest.TestCase):
    def test_a_missing_version_is_refused(self):
        with self.assertRaises(offline_tokenizer.BudgetError):
            discovery_rows.build_identity("25a1717", "   ")

    def test_a_missing_commit_is_refused(self):
        with self.assertRaises(offline_tokenizer.BudgetError):
            discovery_rows.build_identity("", "clio 0.1.0")

    def test_a_complete_identity_is_returned(self):
        self.assertEqual(
            discovery_rows.build_identity(" 25a1717 ", " clio 0.1.0 "),
            {"commit": "25a1717", "version": "clio 0.1.0"},
        )


class RenderTests(unittest.TestCase):
    def test_the_record_cost_comes_from_the_rows_not_an_input(self):
        tokenizer = require_tokenizer(self)
        rows = [
            discovery_rows.row("off", OFF_TOOLS, tokenizer),
            discovery_rows.row("on", ON_TOOLS, tokenizer),
        ]
        output = discovery_rows.render(
            Path("record.json"),
            "minimal",
            "2026-11-07",
            {"commit": "25a1717", "version": "clio 0.1.0"},
            rows,
            tokenizer,
        )
        document = json.loads(output)
        self.assertEqual(
            document["cost"],
            {
                "bytes": document["rows"][1]["bytes"] - document["rows"][0]["bytes"],
                "tokens": document["rows"][1]["tokens"] - document["rows"][0]["tokens"],
            },
        )
        self.assertEqual(document["delivery"], "served-tools-list")
        self.assertEqual(document["preset"], "minimal")


if __name__ == "__main__":
    unittest.main()
