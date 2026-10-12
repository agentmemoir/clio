// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the `batch` tool's
//! command-level `bank` and `actor` defaults: an operation that omits either
//! inherits the batch call's value, and an operation that names its own keeps
//! it.
//!
//! ## Owns
//! - Command-context default and per-operation override assertions for `batch`
//!
//! ## Does not own
//! - In-batch duplicate rejection (see `batch_tools_duplicate_tests`)
//! - General batch orchestration assertions (see `batch_tools_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::batch_tools_test_support_tests::{count_bank, state};
use serde_json::{Value, json};

/// A `store` operation that names neither `bank` nor `actor`.
fn store_op_without_context(id: &str, marker: &str) -> Value {
    json!({
        "tool": "store",
        "params": {
            "item": {
                "id": id,
                "subject_id": format!("subj-{id}"),
                "gist": format!("{marker} prefers terse answers"),
                "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": format!("The user prefers terse answers ({marker}, 1 note since 2024-01-01).")
        }
    })
}

#[test]
fn command_level_bank_and_actor_apply_to_an_operation_that_omits_them() {
    let s = state();
    let out = crate::batch_tools::batch_tool(
        &s,
        &json!({
            "operations": [store_op_without_context("itm-cmd", "sigma")],
            "bank": "agents",
            "actor": "harness"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["operations"][0]["status"], json!("COMMITTED"), "{out}");
    assert_eq!(count_bank(&s, "agents"), 1);
    assert_eq!(
        count_bank(&s, "default"),
        0,
        "the session default bank must not be used"
    );
    let trail = s.store.audit_trail("agents", "itm-cmd").unwrap();
    assert!(
        trail.events.iter().any(|event| event.actor == "harness"),
        "the command actor must attribute the write: {trail:?}"
    );
}

#[test]
fn operation_bank_and_actor_override_the_command_level_values() {
    let s = state();
    let out = crate::batch_tools::batch_tool(
        &s,
        &json!({
            "operations": [{
                "tool": "store",
                "params": {
                    "item": {
                        "id": "itm-op",
                        "subject_id": "subj-op",
                        "gist": "omega prefers terse answers",
                        "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
                    },
                    "category": "persona",
                    "epistemic_kind": "fact",
                    "source_text": "The user prefers terse answers (omega, 1 note since 2024-01-01).",
                    "bank": "bank-a",
                    "actor": "agent"
                }
            }],
            "bank": "agents",
            "actor": "harness"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(count_bank(&s, "bank-a"), 1);
    assert_eq!(count_bank(&s, "agents"), 0);
    let trail = s.store.audit_trail("bank-a", "itm-op").unwrap();
    assert!(
        trail.events.iter().any(|event| event.actor == "agent"),
        "the operation actor must win over the command actor: {trail:?}"
    );
}

#[test]
fn a_null_operation_bank_and_actor_inherit_the_command_level_values() {
    let s = state();
    let mut op = store_op_without_context("itm-null", "nullbank");
    op["params"]["bank"] = Value::Null;
    op["params"]["actor"] = Value::Null;
    let out = crate::batch_tools::batch_tool(
        &s,
        &json!({
            "operations": [op],
            "bank": "agents",
            "actor": "harness"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(count_bank(&s, "agents"), 1);
    assert_eq!(count_bank(&s, "default"), 0);
    let trail = s.store.audit_trail("agents", "itm-null").unwrap();
    assert!(
        trail.events.iter().any(|event| event.actor == "harness"),
        "a null actor must inherit the command actor: {trail:?}"
    );
}

#[test]
fn dry_run_resolves_the_command_level_bank_for_each_operation() {
    let s = state();
    // `shared` is refused when the deployment has not enabled the shared
    // surface. An operation that omits `bank` must inherit the command value
    // and be refused in the dry run; a fallback to the session default would
    // report a successful preview instead.
    let out = crate::batch_tools::batch_tool(
        &s,
        &json!({
            "operations": [store_op_without_context("itm-dry", "dry")],
            "bank": "shared",
            "actor": "harness",
            "dry_run": true
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("forbidden"), "{out}");
    assert_eq!(count_bank(&s, "shared"), 0, "a dry run writes nothing");
}
