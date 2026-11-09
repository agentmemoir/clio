// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the committed `tool_schema` pack to
//! the pack the registry builds today, and for holding the reference-shape
//! candidate to the pack this crate builds when it is asked for that shape, so
//! neither a tool change nor a shape change can land without a fixture change.
//!
//! ## Owns
//! - `the_committed_schema_pack_matches_the_registry`
//! - `the_shipped_pack_holds_no_reference_and_no_definitions`
//! - `the_reference_candidate_keeps_locally_resolved_references`
//! - `the_reference_candidate_can_be_captured`, the gated writer
//! - The committed fixtures' one-tool-per-line layout
//!
//! ## Does not own
//! - The pack document itself (`clio-mcp` `schema`)
//! - Which shape ships (`clio-mcp` `schema_shape`)
//! - Per-tool parameter definitions (`clio-mcp` `schema_defs` and friends)
//!
//! ## Boundary
//!
//! A change to any published tool's parameters, description, or order should
//! normally require both fixtures to be regenerated here. A change to how the
//! pack is assembled should not: this test compares whole documents, not
//! fields.
//!
//! The shipped fixture is embedded with `include_str!` rather than read from
//! disk, so the comparison opens no path and holds no clock or environment
//! input. The one test that writes reads the environment: it is a capture, and
//! it does nothing unless the capture flag is set, so an ordinary run is a pure
//! comparison that passes in any order and in parallel.

use std::path::{Path, PathBuf};

use clio_mcp::schema::{catalog_defs, pack_document, schema_pack, to_export_json};
use clio_mcp::schema_shape::PackShape;
use serde_json::Value;

/// The committed reference-shape candidate, embedded at compile time.
const REFERENCE_PACK: &str = include_str!("fixtures/schema_pack_reference.json");

/// The committed pack, embedded at compile time.
const COMMITTED_PACK: &str = include_str!("fixtures/schema_pack.json");

/// Regeneration instruction carried by the assertion message, so a reader who
/// sees this test fail learns the fix from the failure itself.
const REGENERATE: &str = concat!(
    "the committed schema pack is stale: regenerate it with ",
    "`cargo run --locked -p clio --bin clio -- mcp schema-export --out ",
    "crates/clio-mcp/tests/fixtures/schema_pack.json` and commit the result",
);

/// The flag that turns the reference-candidate capture on. It decides only
/// whether this run writes a fixture; the shipped shape never comes from the
/// environment.
const CAPTURE_ENV: &str = "CLIO_PACK_SHAPE_CAPTURE";

/// Regeneration instruction for the reference candidate's own fixture.
const REGENERATE_REFERENCE: &str = concat!(
    "the reference-shape candidate fixture is stale: regenerate it with ",
    "`CLIO_PACK_SHAPE_CAPTURE=1 cargo test --locked -p clio-mcp ",
    "--test schema_pack_snapshot the_reference_candidate_can_be_captured` ",
    "and commit the result",
);

/// The pack this crate builds for the reference shape, over the whole catalog.
fn reference_pack() -> Value {
    pack_document(&catalog_defs(), PackShape::Reference)
}

/// Every reference a tool's own schema holds resolves inside that schema, so
/// the reference candidate keeps each tool's full local reference closure.
fn assert_every_reference_resolves(pack: &Value) {
    let tools = pack
        .get("tools")
        .and_then(Value::as_array)
        .expect("the reference candidate holds a tools array");
    let mut resolved = 0_usize;
    for tool in tools {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or("?");
        for key in ["inputSchema", "outputSchema"] {
            let Some(schema) = tool.get(key) else {
                continue;
            };
            resolved += count(schema, "$ref");
            assert!(
                clio_config::retention_schema::schema_refs_resolve(schema),
                "tool `{name}` {key} holds a local reference that does not resolve"
            );
        }
    }
    assert!(resolved > 0, "the reference candidate holds a `$ref`");
}

/// The committed reference candidate's path, resolved from the crate root.
fn reference_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("schema_pack_reference.json")
}

/// Guard: a committed fixture keeps one `tools` array element per line, the
/// layout the export writer produces, so a compacted fixture fails the gate.
fn assert_one_tool_per_line(raw: &str) {
    let mut lines = raw.lines();
    lines
        .find(|line| line.trim_start() == "\"tools\": [")
        .expect("the committed pack holds a `tools` array on its own line");
    let mut item_lines = 0_usize;
    let mut closed = false;
    for line in lines {
        if line == "  ]" {
            closed = true;
            break;
        }
        assert!(
            line.starts_with("    "),
            "each tools element must start on its own line with four spaces: {line}"
        );
        let item = line.strip_suffix(',').unwrap_or(line).trim_start();
        let value: Value = serde_json::from_str(item).unwrap_or_else(|error| {
            panic!("each tools line must hold one JSON tool: {error}: {line}")
        });
        assert!(
            value.is_object(),
            "each tools line must hold one tool object: {line}"
        );
        item_lines += 1;
    }
    assert!(closed, "the tools array must close on its own line");
    assert!(item_lines > 0, "the tools array must hold elements");
}

#[test]
fn the_committed_schema_pack_matches_the_registry() {
    assert!(
        COMMITTED_PACK.lines().count() <= 450,
        "the committed schema pack must stay at or under 450 lines"
    );
    let committed: Value = serde_json::from_str(COMMITTED_PACK)
        .expect("crates/clio-mcp/tests/fixtures/schema_pack.json holds JSON");

    assert_eq!(committed, schema_pack(), "{REGENERATE}");
    assert_one_tool_per_line(COMMITTED_PACK);
}

/// How many `keyword` keys appear anywhere in `value`.
fn count(value: &Value, keyword: &str) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.contains_key(keyword))
                + map
                    .values()
                    .map(|child| count(child, keyword))
                    .sum::<usize>()
        }
        Value::Array(items) => items.iter().map(|item| count(item, keyword)).sum(),
        _ => 0,
    }
}

/// The shipped pack resolves every local reference, so it holds neither a
/// `$ref` nor the `$defs` a reference would resolve against: each tool schema
/// is complete on its own, and the committed fixture is that same document.
#[test]
fn the_shipped_pack_holds_no_reference_and_no_definitions() {
    let shipped = schema_pack();
    assert_eq!(
        count(&shipped, "$ref"),
        0,
        "the shipped pack holds no `$ref`"
    );
    assert_eq!(
        count(&shipped, "$defs"),
        0,
        "the shipped pack holds no `$defs`"
    );
    let committed: Value =
        serde_json::from_str(COMMITTED_PACK).expect("the committed pack is JSON");
    assert_eq!(count(&committed, "$ref"), 0, "{REGENERATE}");
    assert_eq!(count(&committed, "$defs"), 0, "{REGENERATE}");
}

/// Capture the reference candidate once, when the flag asks for it, and do
/// nothing at all otherwise. The write is the whole point of this test, so it
/// is the only place here that reads the environment or the filesystem.
#[test]
fn the_reference_candidate_can_be_captured() {
    if std::env::var_os(CAPTURE_ENV).is_none() {
        return;
    }
    let path = reference_fixture_path();
    let text = to_export_json(&reference_pack());
    std::fs::write(&path, text.as_bytes())
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
    let written = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let captured: Value =
        serde_json::from_str(&written).expect("the captured reference candidate is JSON");
    assert_eq!(
        captured,
        reference_pack(),
        "the capture is not a re-render: {REGENERATE_REFERENCE}"
    );
}

/// The reference candidate is the pack this crate builds for that shape, it
/// holds `$ref` and `$defs` with every reference resolving locally, and the
/// shipped pack is still the flattened document that holds neither.
#[test]
fn the_reference_candidate_keeps_locally_resolved_references() {
    let reference: Value = serde_json::from_str(REFERENCE_PACK)
        .expect("crates/clio-mcp/tests/fixtures/schema_pack_reference.json holds JSON");
    assert_eq!(reference, reference_pack(), "{REGENERATE_REFERENCE}");
    assert_one_tool_per_line(REFERENCE_PACK);
    assert!(
        count(&reference, "$ref") > 0,
        "the reference candidate holds a `$ref`"
    );
    assert!(
        count(&reference, "$defs") > 0,
        "the reference candidate holds a `$defs`"
    );
    assert_every_reference_resolves(&reference);

    let shipped = schema_pack();
    let committed: Value =
        serde_json::from_str(COMMITTED_PACK).expect("the committed pack is JSON");
    assert_eq!(shipped, committed, "{REGENERATE}");
    assert_eq!(
        count(&shipped, "$ref"),
        0,
        "the shipped pack holds no `$ref`"
    );
    assert_eq!(
        count(&shipped, "$defs"),
        0,
        "the shipped pack holds no `$defs`"
    );
}
