// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface tests that a supplied `context`
//! reaches stored items through `canonical_put`, `shared_store`, and the
//! transactional `batch` engine.
//!
//! ## Owns
//! - Context passthrough assertions for the store-candidate wrappers
//!   (`canonical_put`, `shared_store`, batch `store`/`canonical_put`)
//!
//! ## Does not own
//! - Plain `store`/`admit_preview` coverage (see `context_write_tests`)
//! - `correct` coverage (see `context_correct_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

fn shared_state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            shared_bank_enabled: true,
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("shared state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

#[test]
fn canonical_put_carries_context_to_the_stored_revision() {
    let s = state();
    let out = call(
        &s,
        "canonical_put",
        json!({
            "key": "primary_database_host",
            "value": "db-a",
            "category": "tool_config",
            "epistemic_kind": "fact",
            "bank": "bank-a",
            "actor": "agent",
            "context": "inventory sheet row 12"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out["pass"].as_bool().unwrap_or(false), "{out}");
    let id = out["id"].as_str().expect("stored id");
    let stored = s
        .store
        .get_memory_item("bank-a", id)
        .expect("read back")
        .expect("stored");
    assert_eq!(stored.context.as_deref(), Some("inventory sheet row 12"));
    assert_eq!(stored.gist.as_deref(), Some("db-a"));
}

#[test]
fn canonical_put_bad_context_fails_without_writing() {
    let s = state();
    let mut args = json!({
        "key": "primary_database_host",
        "value": "db-a",
        "category": "tool_config",
        "epistemic_kind": "fact",
        "bank": "bank-a",
        "actor": "agent"
    });
    args["context"] = json!("");
    let empty = call(&s, "canonical_put", args);
    assert_eq!(empty["ok"], json!(false), "{empty}");
    assert_eq!(empty["code"], json!("invalid_argument"));

    let mut oversized = json!({
        "key": "primary_database_host",
        "value": "db-a",
        "category": "tool_config",
        "epistemic_kind": "fact",
        "bank": "bank-a",
        "actor": "agent"
    });
    oversized["context"] = json!("a".repeat(4097));
    let over = call(&s, "canonical_put", oversized);
    assert_eq!(over["ok"], json!(false), "{over}");
    assert_eq!(over["code"], json!("out_of_range"));
    assert_eq!(
        s.store
            .count_items(&clio_store::ItemInspectFilter::new("bank-a"))
            .expect("count"),
        0
    );
}

#[test]
fn shared_store_preserves_context() {
    let s = shared_state();
    let out = call(
        &s,
        "shared_store",
        json!({
            "item": {
                "id": "itm-shared-ctx",
                "subject_id": "subj-shared",
                "gist": "shared convention"
            },
            "category": "tool_config",
            "epistemic_kind": "fact",
            "actor": "agent",
            "context": "shared rollout note"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let stored = s
        .store
        .get_memory_item("shared", "itm-shared-ctx")
        .expect("read back")
        .expect("stored");
    assert_eq!(stored.context.as_deref(), Some("shared rollout note"));
}

#[test]
fn batch_store_and_canonical_put_carry_context() {
    let s = state();
    let ops = json!([
        {
            "tool": "store",
            "params": {
                "item": {
                    "id": "itm-batch-store",
                    "subject_id": "subj-batch-store",
                    "gist": "prefers terse answers",
                    "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
                },
                "category": "persona",
                "epistemic_kind": "fact",
                "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
                "bank": "bank-a",
                "actor": "agent",
                "context": "batch store context"
            }
        },
        {
            "tool": "canonical_put",
            "params": {
                "item": {"id": "itm-batch-canonical"},
                "key": "primary_database_host",
                "value": "db-b",
                "category": "tool_config",
                "epistemic_kind": "fact",
                "bank": "bank-a",
                "actor": "agent",
                "context": "batch canonical context"
            }
        }
    ]);
    let out = call(&s, "batch", json!({"operations": ops}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let reports = out["operations"].as_array().expect("operation reports");
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["status"], json!("COMMITTED"), "{out}");
    assert_eq!(reports[1]["status"], json!("COMMITTED"), "{out}");

    let stored = s
        .store
        .get_memory_item("bank-a", "itm-batch-store")
        .expect("read back")
        .expect("stored");
    assert_eq!(stored.context.as_deref(), Some("batch store context"));
    let canonical = s
        .store
        .get_memory_item("bank-a", "itm-batch-canonical")
        .expect("read back")
        .expect("stored");
    assert_eq!(
        canonical.context.as_deref(),
        Some("batch canonical context")
    );
}

#[test]
fn batch_rejects_bad_context_and_rolls_back_the_whole_batch() {
    let s = state();
    let ops = json!([
        {
            "tool": "store",
            "params": {
                "item": {
                    "id": "itm-batch-ok",
                    "subject_id": "subj-batch-ok",
                    "gist": "prefers terse answers",
                    "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
                },
                "category": "persona",
                "epistemic_kind": "fact",
                "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
                "bank": "bank-a",
                "actor": "agent"
            }
        },
        {
            "tool": "store",
            "params": {
                "item": {"id": "itm-batch-bad", "subject_id": "s2", "gist": "g2"},
                "category": "persona",
                "epistemic_kind": "fact",
                "bank": "bank-a",
                "actor": "agent",
                "context": ""
            }
        }
    ]);
    let out = call(&s, "batch", json!({"operations": ops}));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(
        s.store
            .count_items(&clio_store::ItemInspectFilter::new("bank-a"))
            .expect("count"),
        0,
        "a malformed context must abort the batch with no partial writes"
    );
}
