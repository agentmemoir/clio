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
//! - The per-tool `bank` property assertions, including the `diagnose` property
//!   its description once declined to publish
//! - The assertions that no operator description still promises the whole store
//!   for an omitted `bank`, in any letter case, or claims the tool takes no
//!   bank scope
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

/// The operator tools that publish a `bank` argument, which is all five: the
/// schema closes each object, so a tool whose arm reads a runtime `bank` has to
/// declare one for a validating client to send it.
const BANK_SCOPED_TOOLS: [&str; 5] = ["diagnose", "verify", "doctor", "repair", "reindex"];

/// Scope wording this branch retired: an omitted `bank` reaching the whole
/// store (or every indexable bank) with no credential condition, an omitted
/// `bank` described as the grant's own bank rather than the server default, a
/// plan's bank promised as the omitted default, and any claim that an operator
/// tool takes no bank scope at all. Matched without regard to letter case.
const SUPERSEDED_CLAIMS: [&str; 8] = [
    "whole store when omitted",
    "every indexable bank when omitted",
    "omitted means the bank the caller's grant covers",
    "resolves to the plan's bank when it names one",
    "Takes no `bank` argument",
    "takes no bank scope",
    "required for explicit actions",
    "Without a bank every action fails",
];

/// The `repair` refusal, verbatim: a plan the credential's grant excludes is
/// answered exactly like an id that was never registered, so no action it
/// carries runs. Pinned as a whole sentence, so a description that promises the
/// plan's own bank instead fails here.
const REPAIR_REFUSAL: &str = "A plan registered for a bank the credential's grant does not \
                              cover is reported as not found, and none of its actions run.";

/// The omitted-`bank` default each scoped tool has to publish: the server's
/// default bank, reachable only when the credential's grant covers it.
const OMITTED_BANK_DEFAULT: &str =
    "omitted means the server default bank, which the credential's grant must cover";

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

/// Every operator tool whose arm reads a runtime `bank` has to publish that
/// argument, because each `inputSchema` closes its object: an omitted property
/// is one a schema-validating client cannot send at all. The clause has to say,
/// in the property itself, that an omitted argument means the server default the
/// grant must cover.
#[test]
fn every_operator_tool_publishes_the_bank_scope_its_arm_reads() {
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
        assert!(
            !tool["inputSchema"]["required"]
                .as_array()
                .is_some_and(|req| req.iter().any(|r| r == "bank")),
            "{name} must not require the bank argument: {tool}"
        );
    }
}

/// A plan registered for a bank outside the credential's grant is answered as
/// not found and none of its actions run, whatever `bank` the request also
/// carries. `repair` has to say that, in those words.
#[test]
fn repair_states_the_out_of_grant_plan_refusal_verbatim() {
    let repair = def("repair");
    let clause = bank_clause(&repair).expect("repair publishes a bank clause");
    assert!(clause.contains(REPAIR_REFUSAL), "repair: {clause}");
}

/// An omitted `bank` is the server default bank for every scoped tool that
/// takes one, and the call is refused when the credential's grant omits it. No
/// tool may describe the default as the grant's own bank instead.
#[test]
fn an_omitted_bank_names_the_server_default_the_grant_must_cover() {
    for name in ["diagnose", "verify", "doctor", "reindex"] {
        let tool_def = def(name);
        let clause = bank_clause(&tool_def).unwrap_or_else(|| panic!("{name} bank clause"));
        assert!(clause.contains(OMITTED_BANK_DEFAULT), "{name}: {clause}");
        assert!(
            !clause.contains("omitted means the bank the caller's grant covers"),
            "{name} still calls an omitted bank the grant's own bank: {clause}"
        );
    }
}

/// `diagnose` reads whatever `bank` the call carries through the same context
/// resolver: a bound caller's `bank` selects inside its grant, while a caller
/// without a credential gets the whole store. It publishes that argument,
/// because the schema closes the object, and states both reaches.
#[test]
fn diagnose_publishes_and_describes_the_bank_its_arm_reads() {
    let diagnose = def("diagnose");
    let clause = bank_clause(&diagnose).expect("diagnose publishes the bank its arm reads");
    assert!(
        clause.contains("without a credential") && clause.contains("whole store"),
        "diagnose must state the reach a caller without a credential keeps: {clause}"
    );
    let description = diagnose.description;
    assert!(
        !description.contains("it does not publish"),
        "diagnose publishes the `bank` argument it reads: {description}"
    );
    assert!(
        description.contains("selects the bank inside its grant"),
        "a bound caller's `bank` narrows diagnose within its grant: {description}"
    );
    assert!(
        description.contains("a caller without a credential gets the whole store"),
        "an unbound caller reaches the whole store: {description}"
    );
}

/// No operator tool may still describe an omitted `bank` as an unconditional
/// whole-store scope, and none may claim it takes no bank scope. Where a
/// description does mention whole-store (or every-indexable-bank) reach, that
/// reach has to be conditioned on the caller holding no credential. Retired
/// wordings are matched without regard to letter case, so a reworded claim in
/// another case still fails.
#[test]
fn no_operator_tool_claims_the_whole_store_for_an_omitted_bank() {
    for name in OPERATOR_TOOLS {
        let text = contract_text(&def(name));
        let lowered = text.to_lowercase();
        for claim in SUPERSEDED_CLAIMS {
            assert!(
                !lowered.contains(&claim.to_lowercase()),
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
