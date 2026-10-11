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

#[test]
fn the_duplicate_waiver_and_evidence_are_declared_where_the_handlers_read_them() {
    // These handlers read the argument, so a client that follows `tools/list` must be able to send it.
    let cases = [
        (
            "persona_observe_preference",
            json!({"key": "k", "observed_value": 0.5, "allow_duplicate": true}),
        ),
        (
            "belief_observe",
            json!({
                "proposition": "p", "confidence": 0.5, "evidence_ref": "e",
                "source_type": "user_stated", "allow_duplicate": true
            }),
        ),
        (
            "import",
            json!({"source": "bundle.json", "allow_duplicate": true}),
        ),
        (
            "import_provider",
            json!({"provider": "mem0", "allow_duplicate": true}),
        ),
        (
            "canonical_put",
            json!({
                "key": "k", "value": "v", "category": "persona",
                "epistemic_kind": "fact", "evidence_ref": "e"
            }),
        ),
    ];
    for (tool, args) in cases {
        assert!(
            check(tool, &args).is_ok(),
            "{tool} must accept its argument"
        );
    }
}

#[test]
fn an_argument_no_handler_acts_on_is_still_refused_by_name() {
    // The handlers once read these and the schema never declared them, so no client could send them.
    // The reads are gone; naming the refusal keeps the surface and the handlers in step.
    let cases = [
        (
            "belief_history",
            "as_of",
            json!({"belief_id_or_proposition": "p", "as_of": "2026-01-01T00:00:00Z"}),
        ),
        (
            "temporal_history",
            "limit",
            json!({"target": "k", "limit": 3}),
        ),
        (
            "task_upsert",
            "subject_id",
            json!({"task_id": "t", "definition": "d", "status": "s", "subject_id": "x"}),
        ),
        (
            "failure_record",
            "subject_id",
            json!({
                "task_id": "t", "attempt_n": 1, "what_failed": "w",
                "lesson": "l", "subject_id": "x"
            }),
        ),
        ("hygiene_clean", "item_id", json!({"item_id": "itm-1"})),
        (
            "sync_ack_skip",
            "remote",
            json!({"sync_mutation_id": "m", "remote": "host:1"}),
        ),
    ];
    for (tool, key, args) in cases {
        let message = refusal(tool, &args);
        assert!(
            message.contains(&format!("unknown argument `{key}` for `{tool}`")),
            "{message}"
        );
    }
}
