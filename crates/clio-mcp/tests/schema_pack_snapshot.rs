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

#[test]
fn the_committed_schema_pack_matches_the_registry() {
    assert!(
        COMMITTED_PACK.lines().count() <= 450,
        "the committed schema pack must stay at or under 450 lines"
    );
    let committed: Value = serde_json::from_str(COMMITTED_PACK)
        .expect("crates/clio-mcp/tests/fixtures/schema_pack.json holds JSON");

    assert_eq!(committed, schema_pack(), "{REGENERATE}");
}
