// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP argument contract: an undeclared argument, a missing
//! required argument, and a blank required string are refused before the tool runs.
//!
//! ## Owns
//! - Assertions over `check` for representative tools from the published schema
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
use serde_json::json;

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
