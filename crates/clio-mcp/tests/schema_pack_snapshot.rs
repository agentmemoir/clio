// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the committed `tool_schema` pack to
//! the pack the registry builds today, so a tool parameter or description
//! change cannot land without the snapshot changing with it.
//!
//! ## Owns
//! - `the_committed_schema_pack_matches_the_registry`
//! - The committed fixture's one-tool-per-line layout
//!
//! ## Does not own
//! - The pack document itself (`clio-mcp` `schema`)
//! - Per-tool parameter definitions (`clio-mcp` `schema_defs` and friends)
//!
//! ## Boundary
//!
//! A change to any published tool's parameters, description, or order should
//! normally require the fixture to be regenerated here. A change to how the
//! pack is assembled should not: this test compares whole documents, not
//! fields.
//
//! The fixture is embedded with `include_str!` rather than read from disk, so
//! the test opens no path, has no working-directory dependency, and holds no
//! clock or environment input. It is a pure comparison and passes in any order
//! and in parallel.

use clio_mcp::schema::schema_pack;
use serde_json::Value;

/// The committed pack, embedded at compile time.
const COMMITTED_PACK: &str = include_str!("fixtures/schema_pack.json");

/// Regeneration instruction carried by the assertion message, so a reader who
/// sees this test fail learns the fix from the failure itself.
const REGENERATE: &str = concat!(
    "the committed schema pack is stale: regenerate it with ",
    "`cargo run --locked -p clio --bin clio -- mcp schema-export --out ",
    "crates/clio-mcp/tests/fixtures/schema_pack.json` and commit the result",
);

/// Guard: the committed fixture keeps one `tools` array element per line, the
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
