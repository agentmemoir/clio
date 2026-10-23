// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the write tool handlers: the timestamp a handler resolves
//! reaches the stored row in the store's canonical form, and a stamp the store
//! cannot place never reaches a row at all.
//!
//! ## Owns
//! - The triple-add handler's stored `tx_from` and item stamp
//! - The invalid-argument refusal of an unplaceable timestamp argument
//!
//! ## Does not own
//! - The resolver itself (`runtime_context_tests`)
//! - Admission decisions or the store's own SQL
//!
//! ## Boundary
//!
//! Tests here break when a handler stops resolving the timestamp through the
//! shared seam; they never change the handler.
//!
//! Keep this module focused on the handler-to-store timestamp hop.

use crate::runtime::McpState;
use crate::write_tools::dispatch;
use serde_json::json;
use std::sync::Arc;

/// One in-memory state; each test builds its own, so no test depends on another.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&crate::runtime::McpOpenOptions::default(), &json!({}))
        .expect("state")
}

#[test]
fn a_fractional_timestamp_reaches_the_triple_row_as_the_second_it_names() {
    let s = state();
    let out = dispatch(
        &s,
        "triple_add",
        &json!({
            "subject": "rust", "predicate": "used_by", "object": "agent",
            "epistemic_kind": "fact", "bank": "bank-a",
            "timestamp": "2026-10-12T08:30:01.500Z"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let rows = s
        .store
        .inspect_items(&clio_store::ItemInspectFilter {
            bank_id: "bank-a".to_owned(),
            category: None,
            epistemic_kind: None,
            source_type: None,
            validity: clio_store::InspectValidity::All,
            time_start: None,
            time_end: None,
            include_discarded: false,
            limit: 10,
            offset: 0,
        })
        .expect("inspect");
    let row = rows.first().expect("the carrier item the write committed");
    assert_eq!(row.created_at, "2026-10-12T08:30:01Z");
    assert_eq!(row.tx_from, "2026-10-12T08:30:01Z");
}

#[test]
fn a_timestamp_the_store_cannot_place_commits_no_row() {
    let s = state();
    let out = dispatch(
        &s,
        "triple_add",
        &json!({
            "subject": "rust", "predicate": "used_by", "object": "agent",
            "epistemic_kind": "fact", "bank": "bank-a",
            "timestamp": "2026-01-01T25:00:00Z"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
    let rows = s
        .store
        .inspect_items(&clio_store::ItemInspectFilter {
            bank_id: "bank-a".to_owned(),
            category: None,
            epistemic_kind: None,
            source_type: None,
            validity: clio_store::InspectValidity::All,
            time_start: None,
            time_end: None,
            include_discarded: false,
            limit: 10,
            offset: 0,
        })
        .expect("inspect");
    assert!(rows.is_empty(), "nothing may be stored: {rows:?}");
}
