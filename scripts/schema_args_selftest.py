#!/usr/bin/env python3
"""Self-test for scripts/schema_args_check.py and scripts/schema_args_rust.py.

Run: python3 scripts/schema_args_selftest.py

The cases read throwaway Rust sources written to a temporary directory, so they
pin how the reader treats each shape without depending on what `clio-mcp`
contains today. The last class runs the check on this checkout, which is the gate
a change to a handler or a schema meets.
"""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import schema_args_check as check  # noqa: E402
import schema_args_rust as rust  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent

READERS = """
pub(crate) fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, E> { args.get(key).unwrap() }
pub(crate) fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> { args.get(key) }
pub(crate) fn opt_bool(args: &Value, key: &str, default: bool) -> bool { default }
"""

HANDLERS = READERS + """
pub fn dispatch(state: &McpState, name: &str, args: &Value) -> Value {
    match name {
        "alpha" => alpha(state, args),
        "beta" => {
            let v = args.get("shared");
            v
        }
        "gamma" | "delta" => forward(state, name, args),
        _ => other(state, name, args),
    }
}
fn alpha(state: &McpState, args: &Value) -> R {
    // args.get("in_a_comment")
    let k = req_str(args, "k")?;
    let flag = opt_bool(args, "undeclared", false);
    let item = args.get("item");
    let inner = item.get("not_a_top_level_key");
    helper(state, args)
}
fn helper(s: &McpState, a: &Value) -> R { opt_str(a, "multi") }
"""

LATE = """
pub fn more(state: &McpState, name: &str, args: &Value) -> Value {
    match name {
        "gamma" => gamma(state, args),
        _ => nothing(),
    }
}
fn gamma(state: &McpState, args: &Value) -> R { opt_str(args, "g") }
"""

BATCH = """
pub fn preflight(state: &McpState, op: &str, params: &Value) -> R {
    match op {
        "alpha" => opt_bool(params, "batch_only", false),
        _ => (),
    }
}
"""

SCHEMA = """
pub fn defs() -> Vec<ToolDef> {
    let read = |name: &'static str, desc: &'static str, extra: Vec<(&'static str, Value)>| ToolDef {
        name,
        description: desc,
        required: &[],
        props: extra.into_iter().chain(bank_actor()).collect(),
    };
    vec![
        ToolDef {
            name: "alpha",
            description: "d",
            required: &["k"],
            props: vec![
                ("k", s("x")),
                ("item", s("i")),
                (
                    "multi",
                    s("y"),
                ),
                context_prop(),
            ]
            .into_iter()
            .chain(bank_actor())
            .collect(),
        },
        factory("beta", "d"),
        read("gamma", "d", vec![("g", s("x"))]),
        read("delta", "d", vec![]),
    ]
}
fn factory(name: &'static str, desc: &'static str) -> ToolDef {
    let props = vec![("shared", s("z"))];
    ToolDef { name, description: desc, required: &[], props }
}
pub(crate) fn bank_actor() -> Vec<(&'static str, Value)> { vec![("bank", s("b")), ("actor", s("a"))] }
pub(crate) fn context_prop() -> (&'static str, Value) { ("context", json!({})) }
"""

REGISTRY = 'pub const T: &[&str] = &[\n    "alpha",\n    "beta",\n    "gamma",\n    "delta",\n    "zeta",\n];\n'


class RustReaderTests(unittest.TestCase):
    def reads(self, source: str, name: str) -> rust.Reads:
        fns = rust.functions(source)
        index: dict[str, list[rust.Fn]] = {}
        for fn in fns:
            index.setdefault(fn.name, []).append(fn)
        readers = {fn.name for fn in fns if rust.is_reader(fn)}
        target = index[name][0]
        return rust.reads_of(target, rust.value_params(target), index, readers)

    def test_comments_are_blanked_and_strings_are_kept(self) -> None:
        text = rust.blank_comments('let a = "// not a comment"; // args.get("no")\n/* args.get("no2") */')
        self.assertIn('"// not a comment"', text)
        self.assertNotIn("args.get", text)

    def test_every_way_of_reading_a_key_is_found_and_nested_objects_are_not(self) -> None:
        source = READERS + """
        fn h(s: &S, args: &Value) -> R {
            let a = req_str(args, "one")?;
            let b = args
                .get("two");
            let c = args["three"];
            let n = args.get("item").get("not_top_level");
            let l = local.get("local");
        }"""
        self.assertEqual(self.reads(source, "h").keys, {"one", "two", "three", "item"})

    def test_a_call_that_passes_the_object_on_is_followed_by_position(self) -> None:
        source = READERS + """
        fn h(s: &S, args: &Value) -> R { inner(s, 3, args) }
        fn inner(s: &S, n: u32, renamed: &Value) -> R { opt_str(renamed, "deep") }
        """
        self.assertEqual(self.reads(source, "h"), rust.Reads({"deep"}, set()))

    def test_a_method_call_skips_self_when_it_maps_the_parameter(self) -> None:
        source = READERS + """
        fn h(s: &S, args: &Value) -> R { s.resolve(args) }
        fn resolve(&self, a: &Value) -> R { opt_str(a, "via_method") }
        """
        self.assertEqual(self.reads(source, "h").keys, {"via_method"})

    def test_an_ambiguous_or_unknown_callee_is_reported_not_guessed(self) -> None:
        source = """
        fn h(args: &Value) -> R { twice(args); mystery(args); }
        fn twice(a: &Value) -> R { 1 }
        mod x { fn twice(a: &Value) -> R { 2 } }
        """
        found = self.reads(source, "h")
        self.assertEqual(found.keys, set())
        self.assertTrue(any("twice" in note for note in found.unresolved), found)
        self.assertIn("mystery", found.unresolved)

    def test_mutual_recursion_terminates(self) -> None:
        source = READERS + """
        fn a(args: &Value) -> R { opt_str(args, "x"); b(args) }
        fn b(args: &Value) -> R { opt_str(args, "y"); a(args) }
        """
        self.assertEqual(self.reads(source, "a").keys, {"x", "y"})

    def test_commas_inside_strings_and_brackets_do_not_split(self) -> None:
        self.assertEqual(rust.split_commas('a, f(b, "x,y"), c[1,2], \'c\''), ["a", 'f(b, "x,y")', "c[1,2]", "'c'"])


def tree(files: dict[str, str]) -> Path:
    root = Path(tempfile.mkdtemp(prefix="schema-args."))
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return root


class CheckTests(unittest.TestCase):
    def setUp(self) -> None:
        src = "crates/clio-mcp/src/"
        self.root = tree(
            {
                src + "handlers.rs": HANDLERS,
                src + "more.rs": LATE,
                src + "batch_ops.rs": BATCH,
                src + "schema_demo_defs.rs": SCHEMA,
                "crates/clio-types/src/mcp_tools.rs": REGISTRY,
            }
        )
        self.addCleanup(lambda: __import__("shutil").rmtree(self.root, ignore_errors=True))

    def test_declared_properties_follow_helpers_factories_and_multiline_tuples(self) -> None:
        index, _ = check.load(check.source_files(self.root))
        declared = check.declared_props(self.root, index)
        self.assertEqual(declared["alpha"], {"k", "item", "multi", "context", "bank", "actor"})
        self.assertEqual(declared["beta"], {"shared"})
        self.assertEqual(declared["gamma"], {"g", "bank", "actor"})
        self.assertEqual(declared["delta"], {"bank", "actor"})

    def test_a_read_the_schema_does_not_declare_is_a_mismatch(self) -> None:
        report = check.check(self.root)
        self.assertEqual({tool: keys for tool, (keys, _) in report.mismatches.items()}, {"alpha": {"undeclared"}})
        self.assertIn("beta", report.checked)

    def test_batch_arms_do_not_count_as_a_tool_handler(self) -> None:
        keys = check.check(self.root).mismatches["alpha"][0]
        self.assertNotIn("batch_only", keys)

    def test_a_tool_that_is_only_forwarded_by_name_is_unchecked_unless_another_arm_handles_it(self) -> None:
        report = check.check(self.root)
        self.assertIn("gamma", report.checked, "an arm in another file handles it")
        self.assertIn("routed by name", report.unchecked["delta"])

    def test_a_registered_tool_with_no_schema_is_unchecked_not_passing(self) -> None:
        report = check.check(self.root)
        self.assertIn("schema was not found", report.unchecked["zeta"])
        self.assertNotIn("zeta", report.checked)

    def test_the_runtime_arguments_are_always_allowed(self) -> None:
        (self.root / "crates/clio-mcp/src/handlers.rs").write_text(
            HANDLERS.replace('"undeclared"', '"timestamp"'), encoding="utf-8"
        )
        self.assertEqual(check.check(self.root).mismatches, {})

    def run_main(self, *argv: str) -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check.main(["--root", str(self.root), *argv])
        return code, out.getvalue()

    def test_the_baseline_allows_a_known_mismatch_and_fails_a_stale_entry(self) -> None:
        with mock.patch.object(check, "KNOWN", {("alpha", "undeclared"): "why"}):
            code, text = self.run_main()
            self.assertEqual(code, 0, text)
            self.assertIn("1 known", text)
            code, text = self.run_main("--no-baseline")
            self.assertEqual(code, 1, text)
            self.assertIn("mismatch: alpha reads `undeclared`", text)
        with mock.patch.object(check, "KNOWN", {("alpha", "undeclared"): "why", ("beta", "gone"): "old"}):
            code, text = self.run_main()
            self.assertEqual(code, 1, text)
            self.assertIn("stale: KNOWN lists beta `gone`", text)

    def test_an_unknown_mismatch_fails_with_an_empty_baseline(self) -> None:
        with mock.patch.object(check, "KNOWN", {}):
            code, text = self.run_main()
        self.assertEqual(code, 1, text)

    def test_a_checkout_without_the_sources_exits_three(self) -> None:
        out = io.StringIO()
        with contextlib.redirect_stderr(out):
            self.assertEqual(check.main(["--root", str(self.root / "nowhere")]), 3)


class ThisCheckoutTests(unittest.TestCase):
    def test_every_registered_tool_is_accounted_for_and_nothing_is_new_or_stale(self) -> None:
        report = check.check(ROOT)
        registry = check.registry_names(ROOT)
        self.assertTrue(registry, "the tool registry was not found")
        self.assertEqual(set(report.checked) | set(report.unchecked), registry)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(check.main([]), 0, out.getvalue())

    def test_each_baseline_entry_carries_one_of_the_two_reasons(self) -> None:
        self.assertTrue(set(check.KNOWN.values()) <= {check.GUARD, check.SHARED})


if __name__ == "__main__":
    unittest.main()
