// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the Core tool schema text that
//! `schema_defs` publishes: the `consolidate` description's automatic
//! maintenance path, its deterministic caller-invoked distinction, and its
//! non-blocking leaf-write clause.
//!
//! ## Owns
//! - The published `consolidate` description assertions
//!
//! ## Does not own
//! - Schema assembly or pack validation (see `schema`)
//! - Per-tool parameter contracts (see `schema_defs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the Core tool schema text.

use super::*;

/// One tool definition from the published schema pack.
fn tool_def(name: &str) -> Value {
    crate::schema::schema_pack()["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("{name} missing"))
        .clone()
}

/// The published `consolidate` description.
fn consolidate_description() -> String {
    tool_def("consolidate")["description"]
        .as_str()
        .expect("consolidate description")
        .to_owned()
}

#[test]
fn consolidate_description_states_the_automatic_maintenance_path() {
    let description = consolidate_description();
    assert!(
        description.contains("also runs automatically"),
        "{description}"
    );
    assert!(
        description.contains("each bank's pressure"),
        "{description}"
    );
    assert!(
        description.contains("thresholds and cooldown are tunable"),
        "{description}"
    );
}

#[test]
fn consolidate_description_states_the_deterministic_tool_distinction() {
    let description = consolidate_description();
    assert!(
        description.contains("deterministic, caller-invoked path"),
        "{description}"
    );
}

#[test]
fn consolidate_description_keeps_the_leaf_write_non_blocking_clause() {
    let description = consolidate_description();
    assert!(
        description.contains("never blocks leaf writes"),
        "{description}"
    );
}
