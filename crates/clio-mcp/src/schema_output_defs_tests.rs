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

const VALID: &str = r#"{
  "format": "am_mcp_output_schemas",
  "format_version": 1,
  "pinned": [],
  "removals": [],
  "tools": [{"name": "store", "schema": {"type": "object"}}]
}"#;

const DUPLICATE: &str = r#"{
  "format": "am_mcp_output_schemas",
  "format_version": 1,
  "pinned": [],
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
