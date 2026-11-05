// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the embedded output-schema
//! registry: parsing, validation, lookup, and tool-entry insertion.
//!
//! ## Owns
//! - Parser acceptance and rejection cases
//! - `outputSchema` insertion and absence
//!
//! ## Does not own
//! - Input-schema definitions (see `schema_defs`)
//! - Pack assembly or export (see `schema`)
//!
//! ## Boundary
//!
//! Changes to registry validation should normally require changes here.
//! Changes to input-schema definitions should not require changes here.
//!
//! Keep this module focused on output-schema registry tests.

use super::*;
use std::collections::BTreeSet;

const VALID: &str = r#"{
  "format": "am_mcp_output_schemas",
  "format_version": 1,
  "pinned": ["store"],
  "removals": [],
  "tools": [{"name": "store", "schema": {"type": "object"}}]
}"#;

const DUPLICATE: &str = r#"{
  "format": "am_mcp_output_schemas",
  "format_version": 1,
  "pinned": ["store"],
  "removals": [],
  "tools": [
    {"name": "store", "schema": {"type": "object"}},
    {"name": "store", "schema": {"type": "object"}}
  ]
}"#;

const BAD_FORMAT: &str = r#"{
  "format": "something_else",
  "format_version": 1,
  "pinned": [],
  "removals": [],
  "tools": []
}"#;

const BAD_VERSION: &str = r#"{
  "format": "am_mcp_output_schemas",
  "format_version": 2,
  "pinned": [],
  "removals": [],
  "tools": []
}"#;

#[test]
fn valid_registry_parses_and_finds_its_tool() {
    let registry = parse_registry(VALID).expect("valid registry");
    let schema = registry.schema_for("store").expect("store schema");
    assert_eq!(schema["type"], "object");
}

#[test]
fn duplicate_tool_name_is_rejected() {
    let error = parse_registry(DUPLICATE).expect_err("duplicate must fail");
    assert!(error.contains("duplicate"), "unexpected error: {error}");
}

#[test]
fn unknown_format_is_rejected() {
    let error = parse_registry(BAD_FORMAT).expect_err("bad format must fail");
    assert!(error.contains("format"), "unexpected error: {error}");
}

#[test]
fn unknown_format_version_is_rejected() {
    let error = parse_registry(BAD_VERSION).expect_err("bad version must fail");
    assert!(error.contains("version"), "unexpected error: {error}");
}

#[test]
fn undeclared_tool_has_no_output_schema() {
    assert!(output_schema("tool_without_declared_output").is_none());
}

#[test]
fn insertion_adds_output_schema_when_present() {
    let mut tool = serde_json::json!({"name": "store"});
    let schema = serde_json::json!({"type": "object", "properties": {}});
    insert_output_schema(&mut tool, Some(&schema));
    assert_eq!(tool["outputSchema"], schema);
}

#[test]
fn insertion_leaves_output_schema_absent_when_missing() {
    let mut tool = serde_json::json!({"name": "store"});
    insert_output_schema(&mut tool, None);
    assert!(tool.get("outputSchema").is_none());
}

/// The committed inventory of MCP tool results, embedded at compile time.
const INVENTORY: &str = include_str!("../../../docs/mcp-tool-result-inventory.md");

/// Tool names read from the inventory table rows.
///
/// Every data row starts with a backticked tool name in its first cell. The
/// parse asserts the expected row count in its caller, so a format change
/// fails instead of silently returning fewer names.
fn inventory_tool_names() -> Vec<&'static str> {
    INVENTORY
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            line.trim_start_matches("| `")
                .split('`')
                .next()
                .expect("an inventory row holds a closing backtick")
        })
        .collect()
}

fn declared_tool_names() -> BTreeSet<&'static str> {
    registry()
        .tools
        .iter()
        .map(|entry| entry.name.as_str())
        .collect()
}

#[test]
fn declared_tools_match_the_inventory() {
    let inventory = inventory_tool_names();
    assert_eq!(
        inventory.len(),
        86,
        "the inventory parse found {} tool rows; its row format changed",
        inventory.len()
    );
    let inventoried: BTreeSet<&str> = inventory.into_iter().collect();
    assert_eq!(
        inventoried.len(),
        86,
        "the inventory rows hold duplicate tool names"
    );
    let declared = declared_tool_names();

    let missing: Vec<&&str> = inventoried.difference(&declared).collect();
    assert!(
        missing.is_empty(),
        "the inventory names tools with no declared output schema: {missing:?}"
    );
    let extra: Vec<&&str> = declared.difference(&inventoried).collect();
    assert!(
        extra.is_empty(),
        "output schemas are declared for tools absent from the inventory: {extra:?}"
    );
}

#[test]
fn every_declared_schema_describes_an_object() {
    for entry in &registry().tools {
        let schema = &entry.schema;
        assert_eq!(
            schema["type"], "object",
            "{} must declare type object at its root",
            entry.name
        );
        assert!(
            schema.get("properties").is_some_and(Value::is_object),
            "{} must declare a properties object",
            entry.name
        );
        assert_ne!(
            schema["additionalProperties"],
            serde_json::json!(false),
            "{} must not close additionalProperties",
            entry.name
        );
    }
}

#[test]
fn pinned_tools_match_declared_tools() {
    let declared = declared_tool_names();
    let pinned: BTreeSet<&str> = registry().pinned.iter().map(String::as_str).collect();

    let unpinned: Vec<&&str> = declared.difference(&pinned).collect();
    assert!(
        unpinned.is_empty(),
        "declared tools missing from pinned: {unpinned:?}"
    );
    let undeclared: Vec<&&str> = pinned.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "pinned names with no declared schema: {undeclared:?}"
    );
}

#[test]
fn to_tool_omits_output_schema_without_a_registry_entry() {
    let def = crate::schema_defs::ToolDef {
        name: "tool_without_declared_output",
        description: "test",
        required: &[],
        props: Vec::new(),
    };
    assert!(def.to_tool().get("outputSchema").is_none());
}
