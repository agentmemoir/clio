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
//! - The `tools/list` entries projected from the declarations
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

use serde_json::{Value, json};
use std::sync::OnceLock;

/// The compiled-in discovery tool-definition document.
const EMBEDDED: &str = include_str!("../discovery_tool_defs.json");

fn parsed() -> &'static Value {
    static DEFINITIONS: OnceLock<Value> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        serde_json::from_str(EMBEDDED)
            .expect("embedded discovery tool definitions are malformed; this is a build defect")
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

/// The four keys a `tools/list` entry publishes, projected from one declaration.
///
/// `argumentRules`, `examples`, and `refusals` stay out of the published entry.
fn published(tool: &Value) -> Value {
    json!({
        "name": tool["name"].clone(),
        "description": tool["description"].clone(),
        "inputSchema": tool["inputSchema"].clone(),
        "outputSchema": tool["outputSchema"].clone(),
    })
}

/// The `tools/list` entry for the declaration named `name`.
///
/// # Panics
///
/// Panics when the embedded document declares no tool named `name`, which is a
/// build defect.
#[must_use]
pub(crate) fn entry(name: &str) -> Value {
    parsed()["tools"]
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
        })
        .map_or_else(
            || {
                panic!(
                    "the embedded discovery definitions declare `{name}`; this is a build defect"
                )
            },
            published,
        )
}

/// Every `tools/list` entry, in document order.
#[must_use]
pub(crate) fn entries() -> Vec<Value> {
    parsed()["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .map(entry)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "discovery_tool_defs_tests.rs"]
mod tests;
