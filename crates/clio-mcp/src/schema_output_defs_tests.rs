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

/// The pinned total of declared output schemas. A tool leaves the inventory
/// only when a removal record keeps this total whole.
const PINNED_SCHEMA_COUNT: usize = 86;

/// Check that the inventory, declared schemas, pinned names, and removal
/// records agree.
fn check_inventory_agreement(
    inventory: &BTreeSet<&str>,
    declared: &BTreeSet<&str>,
    pinned: &BTreeSet<&str>,
    removals: &BTreeSet<&str>,
    expected_total: usize,
) -> Result<(), String> {
    let expected: BTreeSet<&str> = declared.union(removals).copied().collect();
    if *pinned != expected {
        return Err(format!(
            "pinned set must equal declared tools plus removal records; \
             missing from pinned: {:?}; unexpected in pinned: {:?}",
            expected.difference(pinned).collect::<Vec<_>>(),
            pinned.difference(&expected).collect::<Vec<_>>()
        ));
    }
    for removed in removals {
        if declared.contains(removed) {
            return Err(format!("removal record names a declared tool: {removed}"));
        }
        if inventory.contains(removed) {
            return Err(format!(
                "removal record names a tool still in the inventory: {removed}"
            ));
        }
    }
    let total = inventory.len() + removals.len();
    if total != expected_total {
        return Err(format!(
            "inventory rows plus removal records = {total}, expected {expected_total}; \
             a dropped inventory row needs a matching removal record"
        ));
    }
    Ok(())
}

#[test]
fn declared_tools_match_the_inventory() {
    let inventory = inventory_tool_names();
    let inventoried: BTreeSet<&str> = inventory.iter().copied().collect();
    assert_eq!(
        inventoried.len(),
        inventory.len(),
        "the inventory rows hold duplicate tool names"
    );
    let declared = declared_tool_names();
    let pinned: BTreeSet<&str> = registry().pinned.iter().map(String::as_str).collect();
    let removals: BTreeSet<&str> = registry()
        .removals
        .iter()
        .map(|removal| removal.name.as_str())
        .collect();

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
    check_inventory_agreement(
        &inventoried,
        &declared,
        &pinned,
        &removals,
        PINNED_SCHEMA_COUNT,
    )
    .unwrap_or_else(|defect| {
        panic!("the committed registry disagrees with its inventory: {defect}")
    });

    let names: Vec<String> = inventory.iter().map(|name| (*name).to_owned()).collect();
    match plan_declarations(&names, "the committed inventory is not empty")
        .expect("every inventory tool has a declared output schema")
    {
        Declarations::Declared(planned) => {
            let planned: BTreeSet<&str> = planned.iter().map(String::as_str).collect();
            assert_eq!(planned, declared);
        }
        Declarations::NoCode { .. } => panic!("the committed inventory is not empty"),
    }
}

#[test]
fn inventory_agreement_rejects_a_dropped_row_without_a_removal_record() {
    let inventory: BTreeSet<&str> = ["store"].into_iter().collect();
    let declared = inventory.clone();
    let pinned = inventory.clone();
    let removals: BTreeSet<&str> = BTreeSet::new();
    let error = check_inventory_agreement(&inventory, &declared, &pinned, &removals, 2)
        .expect_err("a dropped row with no removal record must fail");
    assert!(
        error.contains("needs a matching removal record"),
        "unexpected error: {error}"
    );
}

#[test]
fn inventory_agreement_rejects_a_pinned_set_that_drops_a_declared_tool() {
    let inventory: BTreeSet<&str> = ["store"].into_iter().collect();
    let declared = inventory.clone();
    let pinned: BTreeSet<&str> = BTreeSet::new();
    let removals: BTreeSet<&str> = BTreeSet::new();
    let error = check_inventory_agreement(&inventory, &declared, &pinned, &removals, 1)
        .expect_err("a pinned set that drops a declared tool must fail");
    assert!(
        error.contains("missing from pinned"),
        "unexpected error: {error}"
    );
}

#[test]
fn inventory_agreement_rejects_a_removal_record_for_a_declared_tool() {
    let inventory: BTreeSet<&str> = ["store", "retired"].into_iter().collect();
    let declared = inventory.clone();
    let pinned = inventory.clone();
    let removals: BTreeSet<&str> = ["retired"].into_iter().collect();
    let error = check_inventory_agreement(&inventory, &declared, &pinned, &removals, 3)
        .expect_err("a removal record for a declared tool must fail");
    assert!(error.contains("declared tool"), "unexpected error: {error}");
}

#[test]
fn inventory_agreement_rejects_a_removal_record_for_a_tool_still_in_the_inventory() {
    let inventory: BTreeSet<&str> = ["store", "retired"].into_iter().collect();
    let declared: BTreeSet<&str> = ["store"].into_iter().collect();
    let pinned: BTreeSet<&str> = ["store", "retired"].into_iter().collect();
    let removals: BTreeSet<&str> = ["retired"].into_iter().collect();
    let error = check_inventory_agreement(&inventory, &declared, &pinned, &removals, 3)
        .expect_err("a removal record for an inventoried tool must fail");
    assert!(
        error.contains("still in the inventory"),
        "unexpected error: {error}"
    );
}

#[test]
fn inventory_agreement_accepts_a_removal_record_that_keeps_the_total() {
    let inventory: BTreeSet<&str> = ["store"].into_iter().collect();
    let declared = inventory.clone();
    let pinned: BTreeSet<&str> = ["store", "retired"].into_iter().collect();
    let removals: BTreeSet<&str> = ["retired"].into_iter().collect();
    check_inventory_agreement(&inventory, &declared, &pinned, &removals, 2)
        .expect("a recorded removal keeps the pinned total");
}

/// Tools that genuinely return only the shared envelope. Empty: every declared
/// schema carries at least one handler-owned payload key.
const ENVELOPE_ONLY_TOOLS: &[&str] = &[];

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
        let has_payload_key = schema["properties"]
            .as_object()
            .expect("properties object")
            .keys()
            .any(|key| !matches!(key.as_str(), "ok" | "code" | "message"));
        assert!(
            has_payload_key || ENVELOPE_ONLY_TOOLS.contains(&entry.name.as_str()),
            "{} declares only envelope keys (ok, code, message); add a payload key \
             or record it in ENVELOPE_ONLY_TOOLS",
            entry.name
        );
    }
}

#[test]
fn pinned_tools_match_declared_tools_plus_removals() {
    let declared = declared_tool_names();
    let removals: BTreeSet<&str> = registry()
        .removals
        .iter()
        .map(|removal| removal.name.as_str())
        .collect();
    let pinned: BTreeSet<&str> = registry().pinned.iter().map(String::as_str).collect();
    let expected: BTreeSet<&str> = declared.union(&removals).copied().collect();

    assert_eq!(
        pinned, expected,
        "pinned names must equal declared tools plus removal records"
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
