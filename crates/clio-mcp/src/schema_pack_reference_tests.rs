// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the committed reference-shape
//! candidate to the pack the crate builds when it is asked for that shape, so
//! the measurement the shape record compares cannot drift from what the
//! builder emits for it.
//!
//! ## Owns
//! - `the_reference_candidate_is_the_pack_the_builder_emits`
//! - `the_reference_candidate_can_be_captured`, the gated writer
//! - The reference candidate fixture's one-tool-per-line layout guard
//!
//! ## Does not own
//! - The pack document itself, or the shape the build ships (`schema`,
//!   `schema_shape`)
//! - The shipped pack's own comparison, or the shape record (`schema_tests`,
//!   the record test outside this crate)
//! - Per-tool parameter definitions (`schema_defs` and friends)
//!
//! ## Boundary
//!
//! A change to the shape builder, or to what a tool declares, should normally
//! require the reference candidate to be captured again. A change to how a
//! measurement is taken should not: this module compares whole documents.
//!
//! This module lives inside the crate because the shape-parameterized builder
//! is crate-private, so only the crate itself can emit the reference candidate.
//! An ordinary run compares and passes in any order; the one test that writes
//! does nothing unless the capture flag is set, and the flag never selects the
//! shape the build ships.

use crate::schema::{catalog_defs, pack_document, to_export_json};
use crate::schema_shape::PackShape;
use serde_json::Value;

/// The committed reference-shape candidate, read from the crate's fixtures.
const FIXTURE_RELATIVE: [&str; 3] = ["tests", "fixtures", "schema_pack_reference.json"];

/// The flag that turns the reference-candidate capture on. It decides only
/// whether this run writes a fixture; the shipped shape never comes from the
/// environment.
const CAPTURE_ENV: &str = "CLIO_PACK_SHAPE_CAPTURE";

/// Regeneration instruction carried by the assertion messages, so a reader who
/// sees this test fail learns the fix from the failure itself.
const REGENERATE: &str = concat!(
    "the reference-shape candidate fixture is stale: regenerate it with ",
    "`CLIO_PACK_SHAPE_CAPTURE=1 cargo test --locked -p clio-mcp --lib ",
    "the_reference_candidate_can_be_captured` and commit the result",
);

/// The pack the crate builds for the reference shape, over the whole catalog.
fn reference_pack() -> Value {
    pack_document(&catalog_defs(), PackShape::Reference)
}

/// The committed reference candidate's path, resolved from the crate root.
fn fixture_path() -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.extend(FIXTURE_RELATIVE);
    path
}

/// The committed reference candidate, as committed.
fn read_fixture() -> String {
    let path = fixture_path();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
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

/// The reference candidate is the pack this crate builds for that shape, it
/// holds `$ref` and `$defs` with every reference resolving locally, and its
/// committed layout is the export writer's.
#[test]
fn the_reference_candidate_is_the_pack_the_builder_emits() {
    let raw = read_fixture();
    let committed: Value =
        serde_json::from_str(&raw).expect("the reference candidate fixture holds JSON");
    assert_eq!(committed, reference_pack(), "{REGENERATE}");
    assert_one_tool_per_line(&raw);
    assert!(
        count(&committed, "$ref") > 0,
        "the reference candidate holds a `$ref`"
    );
    assert!(
        count(&committed, "$defs") > 0,
        "the reference candidate holds a `$defs`"
    );
    assert_every_reference_resolves(&committed);
}

/// Capture the reference candidate once, when the flag asks for it, and do
/// nothing at all otherwise. The write is the whole point of this test, so it
/// is the only place here that reads the environment or writes a file.
#[test]
fn the_reference_candidate_can_be_captured() {
    if std::env::var_os(CAPTURE_ENV).is_none() {
        return;
    }
    let path = fixture_path();
    let text = to_export_json(&reference_pack());
    std::fs::write(&path, text.as_bytes())
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
    let captured: Value =
        serde_json::from_str(&read_fixture()).expect("the captured reference candidate is JSON");
    assert_eq!(
        captured,
        reference_pack(),
        "the capture is not a re-render: {REGENERATE}"
    );
}
