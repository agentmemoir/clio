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

#[cfg(test)]
use std::collections::BTreeSet;

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

/// One recorded decision to withdraw a tool's declared output schema.
#[derive(Debug, Deserialize)]
struct Removal {
    name: String,
    decision_id: String,
    date: String,
    reason: String,
}

impl Removal {
    fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("decision_id", &self.decision_id),
            ("date", &self.date),
            ("reason", &self.reason),
        ] {
            if value.trim().is_empty() {
                return Err(format!(
                    "removal decision for {} has an empty {field}",
                    self.name
                ));
            }
        }
        Ok(())
    }
}

/// Parsed output-schema registry.
#[derive(Debug, Deserialize)]
pub(crate) struct Registry {
    format: String,
    format_version: u32,
    /// Declared output schemas that a later stage keeps in place.
    #[serde(default)]
    pinned: Vec<String>,
    /// Recorded decisions that withdrew a declared output schema.
    #[serde(default)]
    removals: Vec<Removal>,
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

    fn validate(&self) -> Result<(), String> {
        let mut names = HashSet::with_capacity(self.tools.len());
        for entry in &self.tools {
            if !names.insert(entry.name.as_str()) {
                return Err(format!("duplicate output schema for tool: {}", entry.name));
            }
        }
        let pinned: HashSet<&str> = self.pinned.iter().map(String::as_str).collect();
        let mut removed = HashSet::with_capacity(self.removals.len());
        for removal in &self.removals {
            removal.validate()?;
            if !removed.insert(removal.name.as_str()) {
                return Err(format!(
                    "duplicate removal decision for tool: {}",
                    removal.name
                ));
            }
            if names.contains(removal.name.as_str()) {
                return Err(format!(
                    "removal decision names a declared tool: {}",
                    removal.name
                ));
            }
            if !pinned.contains(removal.name.as_str()) {
                return Err(format!(
                    "removal decision names an unpinned tool: {}",
                    removal.name
                ));
            }
        }
        for entry in &self.tools {
            if !pinned.contains(entry.name.as_str()) {
                return Err(format!("declared tool is not pinned: {}", entry.name));
            }
        }
        for name in &self.pinned {
            if !names.contains(name.as_str()) && !removed.contains(name.as_str()) {
                return Err(format!(
                    "pinned tool is no longer declared and has no removal decision: {name}"
                ));
            }
        }
        Ok(())
    }
}

/// Parse and validate a registry document.
///
/// # Errors
/// Returns the defect when the JSON is malformed, the format tag is unknown,
/// the format version is not 1, two entries declare the same tool name, a
/// declared tool is not pinned, a pinned tool is withdrawn without a complete
/// removal decision, or a removal decision names a tool that is still declared
/// or was never pinned.
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
    registry.validate()?;
    Ok(registry)
}

/// The outcome of planning declarations from an inventory.
#[cfg(test)]
#[derive(Debug)]
pub(crate) enum Declarations {
    /// The inventory is empty, so no schema is declared.
    NoCode { reason: String },
    /// The inventory names tools whose schemas are declared.
    Declared(BTreeSet<String>),
}

/// Plan declarations from inventory tool names and the recorded no-code reason.
#[cfg(test)]
pub(crate) fn plan_declarations(
    inventory: &[String],
    no_code_reason: &str,
) -> Result<Declarations, String> {
    if inventory.is_empty() {
        return Ok(Declarations::NoCode {
            reason: no_code_reason.to_owned(),
        });
    }
    let mut declared = BTreeSet::new();
    for name in inventory {
        if registry().schema_for(name).is_none() {
            return Err(format!(
                "inventory names a tool with no declared output schema: {name}"
            ));
        }
        declared.insert(name.clone());
    }
    Ok(Declarations::Declared(declared))
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

#[cfg(test)]
#[path = "schema_output_defs_guard_tests.rs"]
mod schema_output_defs_guard_tests;
