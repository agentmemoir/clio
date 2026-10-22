// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tool-layer tests of the undeclared
//! snapshot-leaf scope rule on the public span-verifying MCP write tools.
//!
//! ## Owns
//! - Proof that `store`, `admit_preview`, `canonical_put`, and `batch` reject
//!   an unlisted snapshot leaf and write nothing
//! - Proof that the tool response carries no caller-supplied snapshot key
//!
//! ## Does not own
//! - Verifier-internal leaf walking and reporting (see `clio-write`)
//! - Other store-path gate ordering tests (see `write_tools_tests`)
//!
//! ## Boundary
//!
//! Changes to the public write-tool snapshot contract should normally require
//! changes here. Changes to unrelated tool dispatch should not.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// A snapshot whose declared leaves all ground, plus one unlisted leaf.
fn args_with_unlisted_leaf(id: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": "subj-1",
            "gist": "prefers terse answers",
            "snapshot": {
                "entity": "terse answers",
                "amount": 1,
                "owner": "platform team"
            }
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": "The user prefers terse answers (1 note).",
        "bank": "bank-a",
        "actor": "agent"
    })
}

#[test]
fn store_rejects_unlisted_snapshot_leaf_and_writes_nothing() {
    let s = state();
    let out = call(&s, "store", args_with_unlisted_leaf("itm-unlisted"));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("admission_refused"), "{out}");
    assert!(!out["pass"].as_bool().unwrap_or(true), "{out}");
    let reason = out["rejection_reason"].as_str().unwrap_or_default();
    assert!(reason.contains("span_verify"), "{out}");
    assert!(reason.contains("unlisted snapshot leaf rejected"), "{out}");
    assert!(
        s.store
            .get_memory_item("bank-a", "itm-unlisted")
            .expect("read back")
            .is_none(),
        "a rejected snapshot must leave no committed item"
    );
    assert!(s.leaves.get("bank-a", "itm-unlisted").is_none());
}

#[test]
fn admit_preview_rejects_unlisted_snapshot_leaf() {
    let s = state();
    let out = call(
        &s,
        "admit_preview",
        args_with_unlisted_leaf("itm-unlisted-pv"),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(!out["pass"].as_bool().unwrap_or(true), "{out}");
    assert!(
        out["rejection_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("span_verify"),
        "{out}"
    );
    assert!(s.leaves.get("bank-a", "itm-unlisted-pv").is_none());
}

#[test]
fn canonical_put_rejects_unlisted_snapshot_leaf() {
    let s = state();
    let out = call(
        &s,
        "canonical_put",
        json!({
            "item": {"id": "itm-unlisted-canon"},
            "key": "primary_database_host",
            "value": "db-1.example.com",
            "snapshot": {"entity": "db-1.example.com", "owner": "platform team"},
            "category": "tool_config",
            "epistemic_kind": "fact",
            "source_text": "The host is db-1.example.com and the owner is the platform team.",
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(!out["pass"].as_bool().unwrap_or(true), "{out}");
    assert!(
        out["rejection_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("unlisted snapshot leaf rejected"),
        "{out}"
    );
}

#[test]
fn batch_store_rejects_unlisted_snapshot_leaf() {
    let s = state();
    let out = call(
        &s,
        "batch",
        json!({"operations": [{
            "tool": "store",
            "params": {
                "item": {
                    "id": "itm-unlisted-batch",
                    "subject_id": "subj-batch",
                    "gist": "prefers terse answers",
                    "snapshot": {"entity": "terse answers", "owner": "platform team"}
                },
                "category": "persona",
                "epistemic_kind": "fact",
                "source_text": "The user prefers terse answers.",
                "bank": "bank-a",
                "actor": "agent"
            }
        }]}),
    );
    // Batch treats a per-operation rejection as fatal and aborts the whole
    // batch, so the rule surfaces in the top-level error and nothing commits.
    assert_eq!(out["ok"], json!(false), "{out}");
    let message = out["message"].as_str().unwrap_or_default();
    assert!(message.contains("span_verify"), "{out}");
    assert!(message.contains("unlisted snapshot leaf rejected"), "{out}");
    assert!(
        s.store
            .get_memory_item("bank-a", "itm-unlisted-batch")
            .expect("read back")
            .is_none(),
        "a rejected snapshot must leave no committed item"
    );
}

#[test]
fn tool_response_and_rejection_never_echo_the_caller_snapshot_key() {
    // The offending key is caller-controlled and may itself be the secret, so
    // neither the tool response nor the rejection reason may carry it.
    let s = state();
    let mut args = args_with_unlisted_leaf("itm-unlisted-secret");
    args["item"]["snapshot"]["sk-live-LEAKEDKEY99"] = json!("x");
    let out = call(&s, "store", args);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(!out["pass"].as_bool().unwrap_or(true), "{out}");
    let response = serde_json::to_string(&out).expect("response serializes");
    assert!(
        !response.contains("sk-live-LEAKEDKEY99"),
        "the tool response leaked the caller key: {response}"
    );
    assert!(
        !response.contains("sk-live"),
        "the tool response leaked a key fragment: {response}"
    );
    // The rule is still named, with a bounded position label.
    let reason = out["rejection_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("<unlisted leaf #"),
        "expected a positional label in: {reason}"
    );
}
