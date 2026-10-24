// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the bank-scope contract the
//! five operator tool schemas publish: the `bank` property each scoped tool
//! carries, the credential grant an omitted `bank` resolves to, and the
//! unconditional whole-store scope wording this branch removed.
//!
//! ## Owns
//! - The per-tool `bank` property assertions, and the argument-free `diagnose`
//!   that names the grant in its description instead
//! - The assertions that no operator description still promises the whole store
//!   for an omitted `bank`, or claims the tool takes no bank scope
//! - The unchanged tool names, empty `required` lists, and `object` roots
//!
//! ## Does not own
//! - The scope an operator call reaches at runtime (`ops_tools_grant_tests`)
//! - Pack assembly or schema validation itself (`schema`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the operator tool schema text.

use super::*;
use crate::schema::validate_tool_schema;
use serde_json::json;

/// The operator tools, in the order the pack publishes them.
const OPERATOR_TOOLS: [&str; 5] = ["diagnose", "verify", "doctor", "repair", "reindex"];

/// The four operator tools that publish a `bank` argument; `diagnose` is the
/// one that binds its scope without one.
const BANK_SCOPED_TOOLS: [&str; 4] = ["verify", "doctor", "repair", "reindex"];

/// Scope wording this branch retired: an omitted `bank` reaching the whole
/// store (or every indexable bank) with no credential condition, and any claim
/// that an operator tool takes no bank scope at all.
const SUPERSEDED_CLAIMS: [&str; 5] = [
    "whole store when omitted",
    "every indexable bank when omitted",
    "takes no bank scope",
    "required for explicit actions",
    "Without a bank every action fails",
];

/// One operator tool definition from the published pack.
fn def(name: &str) -> ToolDef {
    ops_defs()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{name} missing"))
}

/// The description of the tool's `bank` property, when it publishes one.
fn bank_clause(def: &ToolDef) -> Option<&str> {
    def.props
        .iter()
        .find(|(name, _)| *name == "bank")
        .and_then(|(_, schema)| schema["description"].as_str())
}

/// Everything a caller reads about one tool's scope: the tool description and
/// the description of every property it publishes.
fn contract_text(def: &ToolDef) -> String {
    let mut text = def.description.to_owned();
    for (_, schema) in &def.props {
        if let Some(description) = schema["description"].as_str() {
            text.push(' ');
            text.push_str(description);
        }
    }
    text
}

/// A scoped operator tool has to say, in the `bank` clause itself, that an
/// omitted argument means the grant. `diagnose` publishes no such argument, so
/// it has to say the same thing in its own description instead.
#[test]
fn every_operator_tool_publishes_a_bank_scope_or_explains_why_it_takes_none() {
    let scoped: Vec<&str> = OPERATOR_TOOLS
        .iter()
        .copied()
        .filter(|name| bank_clause(&def(name)).is_some())
        .collect();
    assert_eq!(scoped, BANK_SCOPED_TOOLS, "{scoped:?}");

    for name in BANK_SCOPED_TOOLS {
        let tool_def = def(name);
        let clause = bank_clause(&tool_def).unwrap_or_else(|| panic!("{name} bank clause"));
        assert!(clause.contains("grant"), "{name}: {clause}");
        assert!(clause.contains("credential"), "{name}: {clause}");

        let tool = tool_def.to_tool();
        let bank = &tool["inputSchema"]["properties"]["bank"];
        assert_eq!(bank["type"], "string", "{name}: {bank}");
    }

    let diagnose = def("diagnose");
    let description = diagnose.description;
    assert!(
        description.contains("bank"),
        "diagnose must name the argument it declines: {description}"
    );
    assert!(description.contains("grant"), "{description}");
    assert!(
        description.contains("without a credential"),
        "diagnose must say which caller keeps whole-store reach: {description}"
    );
}

/// No operator tool may still describe an omitted `bank` as an unconditional
/// whole-store scope, and none may claim it takes no bank scope. Where a
/// description does mention whole-store (or every-indexable-bank) reach, that
/// reach has to be conditioned on the caller holding no credential.
#[test]
fn no_operator_tool_claims_the_whole_store_for_an_omitted_bank() {
    for name in OPERATOR_TOOLS {
        let text = contract_text(&def(name));
        for claim in SUPERSEDED_CLAIMS {
            assert!(
                !text.contains(claim),
                "{name} still claims `{claim}`: {text}"
            );
        }
        if text.contains("whole store") || text.contains("every indexable bank") {
            assert!(
                text.contains("without a credential"),
                "{name}: only an unbound caller may reach the whole store: {text}"
            );
            assert!(text.contains("grant"), "{name}: {text}");
        }
        if text.contains("omitted") {
            assert!(
                text.contains("grant"),
                "{name} defaults an omitted bank without naming the grant: {text}"
            );
        }
    }
}

/// Every operator tool has to name the credential grant (or the granted banks)
/// where it states its scope, so a caller that omits `bank` knows what it gets.
#[test]
fn every_operator_tool_description_names_the_credential_grant() {
    for name in OPERATOR_TOOLS {
        let def = def(name);
        let scope = bank_clause(&def).unwrap_or(def.description);
        assert!(scope.contains("caller"), "{name}: {scope}");
        assert!(scope.contains("grant"), "{name}: {scope}");
        assert!(
            scope.contains("credential"),
            "{name}: the grant is the credential's, not the argument's: {scope}"
        );
    }
}

/// The contract change touched descriptions only: the five tools stay
/// published in the same order with no required argument, each `inputSchema`
/// still a closed `object` carrying exactly its declared properties.
#[test]
fn operator_tool_names_and_required_fields_are_unchanged() {
    let defs = ops_defs();
    let names: Vec<&str> = defs.iter().map(|d| d.name).collect();
    assert_eq!(names, OPERATOR_TOOLS, "{names:?}");

    for def in &defs {
        assert!(def.required.is_empty(), "{}: {:?}", def.name, def.required);

        let tool = def.to_tool();
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", def.name);
        assert_eq!(tool["inputSchema"]["required"], json!([]), "{}", def.name);
        assert_eq!(
            tool["inputSchema"]["additionalProperties"],
            json!(false),
            "{}",
            def.name
        );

        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{} properties", def.name));
        assert_eq!(properties.len(), def.props.len(), "{}", def.name);
    }
}

/// Every operator schema still validates as a published tool entry: a
/// non-empty description, an `object` root, and resolvable property schemas.
#[test]
fn operator_tool_input_schemas_still_validate_as_objects() {
    for name in OPERATOR_TOOLS {
        let tool = def(name).to_tool();
        assert_eq!(tool["name"], json!(name));
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}: {tool}");
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{name} description"
        );

        let defects = validate_tool_schema(&tool);
        assert!(defects.is_empty(), "{name}: {defects:?}");
    }
}
