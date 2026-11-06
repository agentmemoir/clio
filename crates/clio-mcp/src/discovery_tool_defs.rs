// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for loading the embedded declarations of the
//! discovery tools: `tool_search`, `tool_describe`, and `tool_invoke`.
//!
//! ## Owns
//! - Parsing the embedded discovery tool-definition document
//! - The parsed document served to callers as one JSON value
//!
//! ## Does not own
//! - Registering these tools in the catalog or binding them for dispatch
//! - Handlers, ranking, or accounting for these tools
//! - Input-schema definitions for the business tools (see `schema_defs`)
//!
//! ## Boundary
//!
//! Changes to a discovery tool's declared contract should normally require
//! changes to the embedded data file, not here. Changes to catalog
//! registration or dispatch should not require changes here.
//!
//! Keep this module focused on loading the discovery tool declarations.

use serde_json::Value;
use std::sync::OnceLock;

/// The compiled-in discovery tool-definition document.
const EMBEDDED: &str = include_str!("../discovery_tool_defs.json");

fn parsed() -> &'static Value {
    static DEFINITIONS: OnceLock<Value> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        serde_json::from_str(EMBEDDED).expect(
            "embedded discovery tool definitions are malformed; this is a build defect",
        )
    })
}

/// The parsed discovery tool-definition document.
///
/// # Panics
///
/// Panics if the embedded document is not valid JSON, which is a build defect.
#[must_use]
pub fn definitions() -> &'static Value {
    parsed()
}

#[cfg(test)]
#[path = "discovery_tool_defs_tests.rs"]
mod tests;
