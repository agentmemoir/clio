// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the contract that every command whose MCP tool
//! declares a `bank` property prints `--bank` on its usage line.
//!
//! ## Owns
//! - The registry-versus-schema bank-scope cross-check
//!
//! ## Does not own
//! - The record bytes and parser sets (the tranche test modules)
//! - The schema pack itself (`clio-mcp`'s `schema_pack`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the bank-scope help contract.

use serde_json::Value;

/// MCP tools that select their bank with a different flag, so `--bank` is not
/// the right spelling to advertise: the shared surface uses `--shared-bank` and
/// `summarize` picks a bank or item with `--scope`.
const BANK_OVERRIDE_TOOLS: &[&str] = &[
    "shared_store",
    "shared_retrieve",
    "shared_discard",
    "summarize",
];

/// Every command whose MCP tool declares a `bank` property prints `--bank`,
/// through its documented flags or its authored usage line. The schema pack is
/// the source of truth for which tools are bank-scoped, so a new tool with a
/// `bank` property cannot ship without the flag in its help.
#[test]
fn every_bank_scoped_command_prints_bank() {
    let pack = clio_mcp::schema::schema_pack();
    let tools = pack
        .get("tools")
        .and_then(Value::as_array)
        .expect("the schema pack lists tools");
    let declares_bank = |tool: &str| {
        tools.iter().any(|entry| {
            entry.get("name").and_then(Value::as_str) == Some(tool)
                && entry.pointer("/inputSchema/properties/bank").is_some()
        })
    };
    let mut checked = 0usize;
    let mut missing = Vec::new();
    for spec in crate::cli_command_registry::all() {
        let Some(tool) = spec.tool else { continue };
        if BANK_OVERRIDE_TOOLS.contains(&tool) || !declares_bank(tool) {
            continue;
        }
        checked += 1;
        if !spec.usage_line().contains("--bank") {
            missing.push(spec.name);
        }
    }
    assert!(
        checked >= 40,
        "the schema should mark the bank-scoped commands: checked {checked}"
    );
    assert!(
        missing.is_empty(),
        "bank-scoped commands missing --bank in the usage line: {missing:?}"
    );
}
