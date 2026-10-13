// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the cross-agent
//! shared surface (bank routing + permission gate), the reviewer `validate`
//! tool, and the read-only `stats` reporter.
//!
//! ## Owns
//! - Dispatcher-level assertions for `shared_*`, `validate`, and `stats`
//!
//! ## Does not own
//! - Scratchpad and canonical assertions (see `additive_tools_tests`)
//! - Shared state/dispatch fixtures (see `additive_tools_test_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::additive_tools_test_support_tests::{call, call_read, shared_state, state, store_args};
use clio_types::ErrorCode;
use serde_json::json;

// ---------- shared surface ----------

#[test]
fn shared_surface_disabled_by_default() {
    let s = state();
    let out = call(
        &s,
        "shared_store",
        json!({
            "item": {"id": "itm-sh"}, "epistemic_kind": "fact",
            "category": "schema", "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("forbidden"));
}

#[test]
fn shared_store_routes_and_requires_actor_attribution() {
    let s = shared_state();
    // bank override rejected: shared routes exclusively to `shared`.
    let out = call(
        &s,
        "shared_store",
        json!({
            "item": {"id": "itm-s1", "subject_id": "s1", "gist": "shared convention"},
            "category": "tool_config", "epistemic_kind": "fact",
            "actor": "agent", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));

    let ok = call(
        &s,
        "shared_store",
        json!({
            "item": {"id": "itm-s1", "subject_id": "s1", "gist": "shared convention"},
            "category": "tool_config", "epistemic_kind": "fact", "actor": "agent"
        }),
    );
    assert_eq!(ok["ok"], json!(true), "{ok}");

    // No explicit actor: rejected.
    let no_actor = call(
        &s,
        "shared_store",
        json!({
            "item": {"id": "itm-s2", "gist": "x"}, "category": "tool_config",
            "epistemic_kind": "fact"
        }),
    );
    assert_eq!(no_actor["ok"], json!(false));

    // Retrieve scoped to the shared bank only.
    let hits = call_read(
        &s,
        "shared_retrieve",
        json!({
            "query": "shared convention", "actor": "agent"
        }),
    );
    assert!(hits.is_ok());

    // Discard dry-run then confirm.
    let preview = call(
        &s,
        "shared_discard",
        json!({
            "item_id": "itm-s1", "reason": "convention retired", "actor": "agent"
        }),
    );
    assert_eq!(preview["ok"], json!(true));
    let confirmed = call(
        &s,
        "shared_discard",
        json!({
            "item_id": "itm-s1", "reason": "convention retired", "confirm": true, "actor": "agent"
        }),
    );
    assert_eq!(confirmed["ok"], json!(true));
    assert!(
        s.store
            .get_memory_item("shared", "itm-s1")
            .unwrap()
            .is_some()
    );
}

#[test]
fn shared_store_rejects_invalid_semantic_category() {
    let s = shared_state();
    let out = call(
        &s,
        "shared_store",
        json!({
            "item": {"id": "itm-badcat", "subject_id": "s", "gist": "x"},
            "category": "not_a_category", "epistemic_kind": "fact", "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"));
    assert_eq!(
        s.store
            .count_items(&clio_store::ItemInspectFilter::new("shared"))
            .unwrap(),
        0
    );
}

#[test]
fn shared_bank_is_unreachable_through_generic_tools_when_disabled() {
    let s = state();
    // A read on the shared bank is denied by the routing gate.
    let stats = call_read(&s, "stats", json!({"bank": "shared", "actor": "agent"}));
    assert!(stats.is_err());
    assert_eq!(stats.unwrap_err().code, ErrorCode::Forbidden);
    // A generic `store` targeting the shared bank is denied too.
    let out = call(
        &s,
        "store",
        json!({
            "item": {"id": "itm-gen", "subject_id": "s", "gist": "generic shared write"},
            "category": "tool_config", "epistemic_kind": "fact",
            "bank": "shared", "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("forbidden"));
    assert_eq!(
        s.store
            .count_items(&clio_store::ItemInspectFilter::new("shared"))
            .unwrap(),
        0
    );
}

// ---------- validate ----------

#[test]
fn validate_reviewer_actions() {
    let s = state();
    call(&s, "store", store_args("itm-v"));
    let item_id = "itm-v";

    let attest = call(
        &s,
        "validate",
        json!({
            "item_id": item_id, "action": "attest", "note": "verified", "bank": "bank-a", "actor": "agent"
        }),
    );
    assert_eq!(attest["ok"], json!(true), "{attest}");
    assert_eq!(attest["applied"], false);

    let note_free = call(
        &s,
        "validate",
        json!({
            "item_id": item_id, "action": "attest", "bank": "bank-a", "actor": "agent"
        }),
    );
    assert_eq!(note_free["ok"], json!(true));

    let destructive = call(
        &s,
        "validate",
        json!({
            "item_id": item_id, "action": "invalidate", "bank": "bank-a", "actor": "agent"
        }),
    );
    assert_eq!(destructive["ok"], json!(false));

    let invalidated = call(
        &s,
        "validate",
        json!({
            "item_id": item_id, "action": "invalidate", "note": "stale", "bank": "bank-a", "actor": "agent"
        }),
    );
    assert_eq!(invalidated["ok"], json!(true));
    assert!(
        s.store
            .get_memory_item("bank-a", item_id)
            .unwrap()
            .unwrap()
            .tx_until
            .is_some()
    );
}

// ---------- stats ----------

#[test]
fn stats_counts_stay_bank_scoped() {
    let s = state();
    call(&s, "store", store_args("itm-stats"));
    let report = call_read(&s, "stats", json!({"bank": "bank-a", "actor": "agent"})).unwrap();
    assert_eq!(report["bank"], "bank-a");
    assert!(report["items_total"].as_u64().unwrap_or(0) >= 1);
    let other = call_read(&s, "stats", json!({"actor": "agent"})).unwrap();
    assert_eq!(other["bank"], s.default_bank);
    assert_eq!(other["items_total"], 0);
}
