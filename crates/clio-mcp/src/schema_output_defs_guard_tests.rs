// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the removal guard and the
//! recorded no-code branch of the output-schema registry.
//!
//! ## Owns
//! - Parser rejection of incomplete or contradictory removal decisions
//! - The empty and non-empty inventory planning branches
//!
//! ## Does not own
//! - The embedded registry data (see `output_schemas.json`)
//! - Schema shape or pack assembly (see `schema`)
//!
//! ## Boundary
//!
//! Changes to removal-guard validation should normally require changes here.
//! Changes to schema shape should not require changes here.
//!
//! Keep this module focused on removal-guard and no-code tests.

use super::*;
use serde::Deserialize;
use std::collections::BTreeSet;

const REMOVAL_DATE: &str = "2026-11-05";

fn json_names(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(",")
}

fn registry_json(pinned: &[&str], removals: &[String], tools: &[&str]) -> String {
    let tools: Vec<String> = tools
        .iter()
        .map(|name| format!(r#"{{"name":"{name}","schema":{{"type":"object"}}}}"#))
        .collect();
    format!(
        r#"{{"format":"am_mcp_output_schemas","format_version":1,"pinned":[{}],"removals":[{}],"tools":[{}]}}"#,
        json_names(pinned),
        removals.join(","),
        tools.join(",")
    )
}

fn removal(name: &str, decision_id: &str, date: &str, reason: &str) -> String {
    format!(
        r#"{{"name":"{name}","decision_id":"{decision_id}","date":"{date}","reason":"{reason}"}}"#
    )
}

fn complete_removal(name: &str) -> String {
    removal(name, "dec-1", REMOVAL_DATE, "The tool was retired.")
}

#[test]
fn pinned_tool_without_a_removal_decision_is_rejected() {
    let raw = registry_json(&["retired"], &[], &[]);
    let error = parse_registry(&raw).expect_err("a withdrawn schema needs a decision");
    assert!(
        error.contains("removal decision"),
        "unexpected error: {error}"
    );
}

#[test]
fn removal_with_an_empty_decision_id_is_rejected() {
    let raw = registry_json(
        &["retired"],
        &[removal("retired", "", REMOVAL_DATE, "Retired.")],
        &[],
    );
    let error = parse_registry(&raw).expect_err("an empty decision id must fail");
    assert!(error.contains("decision_id"), "unexpected error: {error}");
}

#[test]
fn removal_with_an_empty_date_is_rejected() {
    let raw = registry_json(
        &["retired"],
        &[removal("retired", "dec-1", "", "Retired.")],
        &[],
    );
    let error = parse_registry(&raw).expect_err("an empty date must fail");
    assert!(error.contains("date"), "unexpected error: {error}");
}

#[test]
fn removal_with_an_empty_reason_is_rejected() {
    let raw = registry_json(
        &["retired"],
        &[removal("retired", "dec-1", REMOVAL_DATE, "")],
        &[],
    );
    let error = parse_registry(&raw).expect_err("an empty reason must fail");
    assert!(error.contains("reason"), "unexpected error: {error}");
}

#[test]
fn removal_missing_a_field_is_rejected() {
    let raw = registry_json(
        &["retired"],
        &[format!(
            r#"{{"name":"retired","date":"{REMOVAL_DATE}","reason":"Retired."}}"#
        )],
        &[],
    );
    let error = parse_registry(&raw).expect_err("a missing field must fail");
    assert!(error.contains("JSON"), "unexpected error: {error}");
}

#[test]
fn removal_for_a_still_declared_tool_is_rejected() {
    let raw = registry_json(&["store"], &[complete_removal("store")], &["store"]);
    let error = parse_registry(&raw).expect_err("a live tool cannot be removed");
    assert!(error.contains("declared tool"), "unexpected error: {error}");
}

#[test]
fn removal_for_an_unpinned_tool_is_rejected() {
    let raw = registry_json(&["store"], &[complete_removal("other")], &["store"]);
    let error = parse_registry(&raw).expect_err("an unpinned tool cannot be removed");
    assert!(error.contains("unpinned tool"), "unexpected error: {error}");
}

#[test]
fn declared_tool_missing_from_pinned_is_rejected() {
    let raw = registry_json(&[], &[], &["store"]);
    let error = parse_registry(&raw).expect_err("every declared tool must be pinned");
    assert!(error.contains("not pinned"), "unexpected error: {error}");
}

#[test]
fn recorded_removal_permits_withdrawal() {
    let raw = registry_json(
        &["store", "retired"],
        &[complete_removal("retired")],
        &["store"],
    );
    let registry = parse_registry(&raw).expect("a complete removal decision is valid");
    assert!(registry.schema_for("store").is_some());
    assert!(registry.schema_for("retired").is_none());
}

#[derive(Deserialize)]
struct InventoryFixture {
    inventory: Vec<String>,
    reason: String,
}

const EMPTY_FIXTURE: &str = include_str!("../tests/fixtures/output_schema_inventory_empty.json");
const NONEMPTY_FIXTURE: &str =
    include_str!("../tests/fixtures/output_schema_inventory_nonempty.json");

fn fixture(raw: &str) -> InventoryFixture {
    serde_json::from_str(raw).expect("inventory fixture JSON")
}

#[test]
fn empty_inventory_yields_the_recorded_no_code_branch() {
    let fixture = fixture(EMPTY_FIXTURE);
    let declarations =
        plan_declarations(&fixture.inventory, &fixture.reason).expect("an empty inventory");
    match declarations {
        Declarations::NoCode { reason } => {
            assert!(
                !reason.trim().is_empty(),
                "the no-code reason must be recorded"
            );
            assert_eq!(reason, fixture.reason);
        }
        Declarations::Declared(_) => panic!("an empty inventory must declare no schema"),
    }
}

#[test]
fn nonempty_inventory_declares_only_registry_tools() {
    let fixture = fixture(NONEMPTY_FIXTURE);
    assert!(
        fixture.inventory.len() >= 3,
        "the fixture must name at least three tools"
    );
    let declarations =
        plan_declarations(&fixture.inventory, &fixture.reason).expect("a non-empty inventory");
    match declarations {
        Declarations::Declared(declared) => {
            let expected: BTreeSet<String> = fixture.inventory.iter().cloned().collect();
            assert_eq!(declared, expected);
            for name in &declared {
                assert!(
                    output_schema(name).is_some(),
                    "{name} must be in the registry"
                );
            }
        }
        Declarations::NoCode { .. } => panic!("a non-empty inventory must declare schemas"),
    }
}

#[test]
fn inventory_name_absent_from_registry_is_an_error() {
    let error = plan_declarations(&["not_a_registry_tool".to_owned()], "A reason.")
        .expect_err("an unknown name must fail");
    assert!(
        error.contains("no declared output schema"),
        "unexpected error: {error}"
    );
}
