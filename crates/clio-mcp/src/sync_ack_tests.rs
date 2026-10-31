// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP `sync_ack_skip` tool (`sync_ack.rs`): the dry-run
//! and confirmed shapes, the missing-id refusal, the gate that refuses a
//! mutation whose own bank is outside the credential's grant, and the
//! acknowledgement of one inside it.
//!
//! ## Owns
//! - The ack-skip result shapes over an in-memory sqlite fixture
//! - The grant refusal and the in-grant ack
//!
//! ## Does not own
//! - The per-bank gate itself (`runtime_context`)
//! - The other sync surfaces (`sync_tools_tests`, `sync_tools_grant_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the ack-skip surface or its bank gate changes; they
//! never change it.

use crate::principal::Principal;
use crate::runtime::McpState;
use crate::sync_tools::sync_tools_test_support_tests::state;
use crate::write_tools::dispatch;
use clio_store::{DeadLetterStatus, SyncDeadLetterRow};
use serde_json::{Value, json};

/// One state whose credential covers only `banks`.
fn bound(banks: &[&str]) -> McpState {
    state().with_principal(Principal::new(
        "sync-runner".into(),
        clio_types::Actor::Harness,
        banks.iter().map(|bank| (*bank).to_owned()).collect(),
    ))
}

/// Open one dead letter in `bank`, so an ack-skip has a row to reach.
fn open_dead_letter(st: &McpState, id: &str, bank: &str) {
    st.sync
        .dead_letter_open(&SyncDeadLetterRow {
            sync_mutation_id: id.into(),
            bank_id: bank.into(),
            reason_code: "malformed".into(),
            detail: "fixture".into(),
            envelope_json: "{}".into(),
            status: DeadLetterStatus::Open,
            first_seen: "2026-01-01T00:00:00Z".into(),
            last_error: None,
            updated_at: None,
        })
        .expect("the dead letter opens");
}

#[test]
fn ack_skip_requires_confirm_and_shapes_both_paths() {
    let st = state();
    // Dry-run without confirm.
    let dry = dispatch(
        &st,
        "sync_ack_skip",
        &json!({ "sync_mutation_id": "mut-1" }),
    );
    assert_eq!(dry["ok"], Value::Bool(true), "{dry}");
    assert_eq!(dry["acked"], Value::Bool(false));
    assert_eq!(dry["would_ack"], "mut-1");
    // Confirmed: unknown id acks false but the shape holds.
    let confirmed = dispatch(
        &st,
        "sync_ack_skip",
        &json!({ "sync_mutation_id": "mut-unknown", "confirm": true, "reason": "poison" }),
    );
    assert_eq!(confirmed["ok"], Value::Bool(true), "{confirmed}");
    assert!(confirmed["acked"].is_boolean());
    assert_eq!(confirmed["sync_mutation_id"], "mut-unknown");
    assert_eq!(confirmed["reason"], "poison");
    // Missing id fails closed.
    let missing = dispatch(&st, "sync_ack_skip", &json!({ "confirm": true }));
    assert_eq!(missing["ok"], Value::Bool(false), "{missing}");
}

/// An ack-skip resolves the mutation's own bank and gates it: a dead letter in
/// a foreign bank is refused and stays open, never acked.
#[test]
fn ack_skip_refuses_a_mutation_whose_bank_is_outside_the_grant() {
    let st = bound(&["b-allowed"]);
    open_dead_letter(&st, "m-foreign", "b-foreign");
    let out = dispatch(
        &st,
        "sync_ack_skip",
        &json!({ "sync_mutation_id": "m-foreign", "confirm": true }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{out}");
    assert_eq!(
        st.sync.dead_letter_status("m-foreign").expect("status"),
        Some(DeadLetterStatus::Open),
        "a refused ack leaves the dead letter open"
    );
}

/// A dead letter inside the grant is acknowledged: the gate does not block the
/// one mutation the credential may reach.
#[test]
fn ack_skip_acks_a_mutation_inside_the_grant() {
    let st = bound(&["b-allowed"]);
    open_dead_letter(&st, "m-inside", "b-allowed");
    let out = dispatch(
        &st,
        "sync_ack_skip",
        &json!({ "sync_mutation_id": "m-inside", "confirm": true }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["acked"], Value::Bool(true), "{out}");
    assert_eq!(
        st.sync.dead_letter_status("m-inside").expect("status"),
        Some(DeadLetterStatus::AckedSkip),
        "the granted dead letter is acked"
    );
}
