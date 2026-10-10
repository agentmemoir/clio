// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `batch` MCP-surface coverage of the
//! canonical single-slot op (in-transaction supersession of two puts on one
//! key, snapshot passthrough) and the shared-bank permission boundary.
//!
//! ## Owns
//! - Canonical-in-batch and shared-bank boundary assertions for `batch`
//!
//! ## Does not own
//! - General batch orchestration assertions (see `batch_tools_tests`)
//! - Shared state/dispatch fixtures (see `batch_tools_test_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::batch_tools_test_support_tests::{batch, count_bank, state};
use serde_json::json;

#[test]
fn two_canonical_puts_one_key_in_one_batch_leave_one_active_revision() {
    let s = state();
    let ops = json!([
        {"tool": "canonical_put", "params": {
            "key": "primary_host", "value": "db-a", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent",
            "timestamp": "2026-01-01T00:00:00Z"
        }},
        {"tool": "canonical_put", "params": {
            "key": "primary_host", "value": "db-b", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent",
            "timestamp": "2026-01-02T00:00:00Z"
        }}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");

    let active = crate::read_tools::dispatch(
        &s,
        "canonical_get",
        &json!({"key": "primary_host", "bank": "bank-a", "actor": "agent"}),
    )
    .unwrap();
    assert_eq!(active["value"], "db-b");
    // The first revision is closed on both axes, not left active.
    let historical = crate::read_tools::dispatch(
        &s,
        "canonical_get",
        &json!({
            "key": "primary_host", "as_of": "2026-01-01T12:00:00Z",
            "bank": "bank-a", "actor": "agent"
        }),
    )
    .unwrap();
    assert_eq!(historical["value"], "db-a");
    assert_ne!(active["item_id"], historical["item_id"]);
    let prior_id = historical["item_id"].as_str().unwrap();
    let prior = s
        .store
        .get_memory_item("bank-a", prior_id)
        .unwrap()
        .unwrap();
    assert_eq!(prior.tx_until.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert!(prior.valid_until.is_some());
    // One active + one superseded row; no third row.
    assert_eq!(count_bank(&s, "bank-a"), 2);
    let subject = clio_compliance::canonical_subject("primary_host");
    let current = s
        .store
        .get_item_by_subject("bank-a", &subject, None)
        .unwrap()
        .unwrap();
    assert_eq!(current.gist.as_deref(), Some("db-b"));
    assert!(current.tx_until.is_none());
}

#[test]
fn canonical_put_in_batch_passes_snapshot_through() {
    let s = state();
    let ops = json!([{"tool": "canonical_put", "params": {
        "key": "host", "value": "db-a", "category": "tool_config",
        "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent",
        "snapshot": {"entity": "db-a", "amount": 1, "date": "2024-01-01"},
        "source_text": "db-a (1 note since 2024-01-01)."
    }}]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");
    let read = crate::read_tools::dispatch(
        &s,
        "canonical_get",
        &json!({"key": "host", "bank": "bank-a", "actor": "agent"}),
    )
    .unwrap();
    assert_eq!(read["snapshot"]["entity"], json!("db-a"));
}

#[test]
fn batch_cannot_reach_shared_bank_when_surface_disabled() {
    let s = state();
    let ops = json!([{
        "tool": "store",
        "params": {
            "item": {
                "id": "itm-sh", "subject_id": "s", "gist": "shared via batch",
                "snapshot": {"entity": "x", "amount": 1, "date": "2024-01-01"}
            },
            "category": "tool_config", "epistemic_kind": "fact",
            "source_text": "x (1 note since 2024-01-01).",
            "bank": "shared", "actor": "agent"
        }
    }]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("forbidden"));
    assert_eq!(count_bank(&s, "shared"), 0);
    // The shared_* wrapper still reports forbidden (surface disabled).
    let wrapper = crate::write_tools::dispatch(
        &s,
        "shared_store",
        &json!({
            "item": {"id": "itm-sh2", "gist": "x"}, "category": "tool_config",
            "epistemic_kind": "fact", "actor": "agent"
        }),
    );
    assert_eq!(wrapper["code"], json!("forbidden"));
}
