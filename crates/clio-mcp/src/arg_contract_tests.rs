// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP argument contract: an undeclared argument, a wrongly
//! typed argument, a missing required argument, and a blank required string are
//! refused before the tool runs.
//!
//! ## Owns
//! - Assertions over `check` for representative tools from the published schema
//! - The schema-driven sweep that every published property type is enforced
//!
//! ## Does not own
//! - The tool handlers' own value rules (see each tool's test module)
//!
//! ## Boundary
//!
//! Changes to the contract rules should normally require changes here. Changes to
//! a tool's accepted arguments should not require changes here beyond its name.
//!
//! Keep this module focused on the argument contract.

use super::check;
use serde_json::{Map, Value, json};

/// The message text, or a panic when the check unexpectedly passes.
fn refusal(name: &str, args: &serde_json::Value) -> String {
    check(name, args)
        .expect_err("the contract should refuse these arguments")
        .message
}

#[test]
fn an_undeclared_argument_is_refused_by_name() {
    // A `store` that carries `text` instead of `item` used to store an empty memory.
    let message = refusal("store", &json!({"text": "hello", "epistemic_kind": "fact"}));
    assert!(
        message.contains("unknown argument `text` for `store`"),
        "{message}"
    );
}

#[test]
fn a_missing_required_argument_is_refused() {
    let message = refusal("store", &json!({"epistemic_kind": "fact"}));
    assert_eq!(message, "item is required");
}

#[test]
fn an_empty_required_string_keeps_the_handler_wording() {
    let message = refusal(
        "canonical_put",
        &json!({"key": "k", "value": "", "category": "persona", "epistemic_kind": "fact"}),
    );
    assert_eq!(message, "value is required");
}

#[test]
fn a_whitespace_only_required_string_is_refused() {
    for blank in ["   ", "\t\n"] {
        let message = refusal(
            "canonical_put",
            &json!({"key": "k", "value": blank, "category": "persona", "epistemic_kind": "fact"}),
        );
        assert_eq!(message, "value must not be empty or whitespace-only");
    }
}

#[test]
fn a_whitespace_only_string_in_a_triple_is_refused() {
    let message = refusal(
        "triple_add",
        &json!({"subject": "   ", "predicate": "p", "object": "o", "epistemic_kind": "fact"}),
    );
    assert_eq!(message, "subject must not be empty or whitespace-only");
}

#[test]
fn a_call_with_every_declared_argument_passes() {
    let args = json!({
        "subject": "s",
        "predicate": "p",
        "object": "o",
        "epistemic_kind": "fact"
    });
    assert!(check("triple_add", &args).is_ok());
}

#[test]
fn a_tool_without_a_published_contract_is_left_to_its_handler() {
    assert!(check("no_such_tool", &json!({"anything": 1})).is_ok());
}

#[test]
fn an_optional_blank_string_is_not_a_contract_fault() {
    // `bank` is optional on every tool. The handler decides what a blank bank means.
    assert!(check("stats", &json!({"bank": "   "})).is_ok());
}

#[test]
fn dry_run_is_declared_on_the_record_tools_whose_handlers_honor_it() {
    // These handlers read `dry_run` and have a no-write path, so the schema must declare it.
    let cases = [
        (
            "task_upsert",
            json!({"task_id": "t", "definition": "d", "status": "s", "dry_run": true}),
        ),
        (
            "failure_record",
            json!({"task_id": "t", "attempt_n": 1, "what_failed": "w", "lesson": "l", "dry_run": true}),
        ),
        (
            "persona_put_stable",
            json!({"key": "k", "value": "v", "dry_run": true}),
        ),
    ];
    for (tool, args) in cases {
        assert!(check(tool, &args).is_ok(), "{tool} must accept dry_run");
    }
}

/// One JSON value of each type the catalog declares.
fn sample(type_name: &str) -> Option<Value> {
    match type_name {
        "string" => Some(json!("x")),
        "number" | "integer" => Some(json!(1)),
        "boolean" => Some(json!(true)),
        "array" => Some(json!([])),
        "object" => Some(json!({})),
        _ => None,
    }
}

/// The `type` names a property schema declares, in the string form or the array
/// form. A property with no `type` declares none, and is not type-checked.
fn declared_names(prop: &Value) -> Vec<&str> {
    match prop.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// A value the declared type set admits, and one it does not.
fn typed_pair(names: &[&str]) -> Option<(Value, Value)> {
    let good = names.iter().find_map(|name| sample(name))?;
    let bad = ["string", "boolean", "number", "object", "array"]
        .iter()
        .find(|name| !names.contains(name))
        .and_then(|name| sample(name))?;
    Some((good, bad))
}

#[test]
fn a_wrong_typed_optional_argument_is_refused_by_name() {
    // Reported over MCP stdio: a string `limit` read as absent, so the call
    // returned the default page and reported success.
    let error = check("retrieve", &json!({"query": "q", "limit": "not_a_number"}))
        .expect_err("a string limit must be refused");
    assert_eq!(error.code.as_str(), "invalid_type");
    assert_eq!(error.message, "limit must be a number");
    // A whole number is what the schema types `limit` as, and it still passes.
    assert!(check("retrieve", &json!({"query": "q", "limit": 7})).is_ok());
}

#[test]
fn an_explicit_null_reads_as_absent_and_is_not_type_checked() {
    // Every argument reader in this crate reads an explicit null as absent, so
    // null stays a documented way to take the default rather than a type fault.
    assert!(check("retrieve", &json!({"query": "q", "limit": null})).is_ok());
}

#[test]
fn a_polymorphic_argument_keeps_the_forms_its_handler_accepts() {
    // `correct` serializes any JSON value into the corrected gist/snapshot, and
    // `update` stringifies a number. The published schema has to admit both
    // forms, or the contract would refuse a call the handler serves.
    assert!(
        check(
            "correct",
            &json!({"item_id": "i", "reason": "r", "new_value": {"meeting": "3pm"}})
        )
        .is_ok()
    );
    assert!(
        check(
            "update",
            &json!({"target": "t", "new_value": 0.8, "update_rule": "continuous"})
        )
        .is_ok()
    );
    // The declared set is enforced narrowly: a type it excludes is still refused.
    let error = check(
        "update",
        &json!({"target": "t", "new_value": {}, "update_rule": "continuous"}),
    )
    .expect_err("an object new_value must be refused");
    assert_eq!(error.message, "new_value must be a string or a number");
}

#[test]
fn every_published_property_type_is_enforced() {
    let published = crate::protocol::McpHandler::tools_list();
    let tools = published["tools"].as_array().expect("tools array").clone();
    assert!(!tools.is_empty(), "the published pack must not be empty");
    let mut typed_tools = 0_usize;
    let mut refusals = 0_usize;
    for tool in &tools {
        let name = tool["name"].as_str().expect("tool name");
        let props = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} publishes no properties"));
        // Carry a value each property's own type admits — null for a property
        // the schema deliberately leaves untyped, so it still reads as present —
        // so when one property is flipped the only fault left is the type under
        // test.
        let good: Map<String, Value> = props
            .iter()
            .map(|(key, prop)| {
                let value = typed_pair(&declared_names(prop)).map_or(Value::Null, |(ok, _)| ok);
                (key.clone(), value)
            })
            .collect();
        let mut tool_refusals = 0_usize;
        for (key, prop) in props {
            let Some((_, bad)) = typed_pair(&declared_names(prop)) else {
                continue;
            };
            let mut args = good.clone();
            args.insert(key.clone(), bad);
            let error = check(name, &Value::Object(args))
                .expect_err(&format!("{name}: a mistyped `{key}` must be refused"));
            assert_eq!(
                error.code.as_str(),
                "invalid_type",
                "{name}.{key}: {}",
                error.message
            );
            assert!(
                error.message.starts_with(&format!("{key} must be ")),
                "{name}.{key}: {}",
                error.message
            );
            tool_refusals += 1;
        }
        if tool_refusals > 0 {
            typed_tools += 1;
            refusals += tool_refusals;
        }
    }
    // A handful of published views declare no property at all, so the sweep
    // cannot cover every tool; it must still cover the rest of the catalog,
    // which declares far more typed properties than there are tools.
    assert!(
        refusals >= tools.len(),
        "the sweep must exercise the type rule across the catalog: {refusals} refusal(s) over \
         {typed_tools} of {} tool(s)",
        tools.len()
    );
}
