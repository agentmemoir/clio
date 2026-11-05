// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the embedded registry of per-tool output
//! schemas and for attaching those schemas to published tool entries.
//!
//! ## Owns
//! - Parsing and validating the embedded output-schema registry
//! - The `outputSchema` lookup for a tool name
//! - Insertion of `outputSchema` into a published tool entry
//!
//! ## Does not own
//! - Input-schema definitions (see `schema_defs`)
//! - Pack assembly or export (see `schema`)
//!
//! ## Boundary
//!
//! Changes to an output schema's shape should normally require changes to the
//! registry data file, not here. Changes to input schemas or pack assembly
//! should not require changes here.
//!
//! Keep this module focused on output-schema registry concerns.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Registry format tag accepted by the parser.
const FORMAT: &str = "am_mcp_output_schemas";

/// Registry format version accepted by the parser.
const FORMAT_VERSION: u32 = 1;

/// The compiled-in registry data.
const EMBEDDED: &str = include_str!("../output_schemas.json");

/// One declared tool and its output schema.
#[derive(Debug, Deserialize)]
struct ToolEntry {
    name: String,
    schema: Value,
}

/// Parsed output-schema registry.
#[derive(Debug, Deserialize)]
pub(crate) struct Registry {
    format: String,
    format_version: u32,
    /// Declared output schemas that a later stage keeps in place.
    #[serde(default)]
    #[allow(dead_code)]
    pinned: Vec<String>,
    /// Declared output schemas that a later stage removes.
    #[serde(default)]
    #[allow(dead_code)]
    removals: Vec<String>,
    #[serde(default)]
    tools: Vec<ToolEntry>,
}

impl Registry {
    fn schema_for(&self, name: &str) -> Option<&Value> {
        self.tools
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| &entry.schema)
    }
}

/// Parse and validate a registry document.
///
/// # Errors
/// Returns the defect when the JSON is malformed, the format tag is unknown,
/// the format version is not 1, or two entries declare the same tool name.
pub(crate) fn parse_registry(raw: &str) -> Result<Registry, String> {
    let registry: Registry = serde_json::from_str(raw)
        .map_err(|error| format!("invalid output-schema registry JSON: {error}"))?;
    if registry.format != FORMAT {
        return Err(format!(
            "unsupported output-schema registry format: {}",
            registry.format
        ));
    }
    if registry.format_version != FORMAT_VERSION {
        return Err(format!(
            "unsupported output-schema registry format version: {}",
            registry.format_version
        ));
    }
    let mut names = HashSet::with_capacity(registry.tools.len());
    for entry in &registry.tools {
        if !names.insert(entry.name.as_str()) {
            return Err(format!("duplicate output schema for tool: {}", entry.name));
        }
    }
    Ok(registry)
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        parse_registry(EMBEDDED)
            .expect("embedded output-schema registry is malformed; this is a build defect")
    })
}

/// The declared output schema for `tool`, when one exists.
pub(crate) fn output_schema(tool: &str) -> Option<&'static Value> {
    registry().schema_for(tool)
}

/// Insert `outputSchema` into `tool` when `schema` is present.
pub(crate) fn insert_output_schema(tool: &mut Value, schema: Option<&Value>) {
    if let (Some(schema), Value::Object(fields)) = (schema, tool) {
        fields.insert("outputSchema".to_owned(), schema.clone());
    }
}

#[cfg(test)]
#[path = "schema_output_defs_tests.rs"]
mod schema_output_defs_tests;
