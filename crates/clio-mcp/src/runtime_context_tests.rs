// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the per-call context resolution of `runtime_context.rs`:
//! which credential label a resolved context carries, and the two refusals that
//! run before it is built.
//!
//! ## Owns
//! - Test coverage for the bound-versus-local principal label on `resolve_ctx`
//! - Test coverage for the actor-conflict and out-of-list-bank refusals
//! - Test coverage for the UTC stamp helper at a known second
//!
//! ## Does not own
//! - The identity rule itself (`principal.rs`)
//! - Tool dispatch and the shared-bank deployment gate
//!
//! ## Boundary
//!
//! Tests here break when context resolution changes; they never change it.
//!
//! Keep this module focused on `resolve_ctx` and the per-call helpers.

use super::*;
use crate::principal::Principal;
use crate::runtime::McpOpenOptions;
use serde_json::json;
use std::sync::Arc;

/// One in-memory state; each test builds its own, so no test depends on another.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// A state scoped to one credential that may reach `bank-a` as `harness`.
fn bound(label: &str) -> Arc<McpState> {
    Arc::new(state().with_principal(Principal::new(
        label.to_owned(),
        Actor::Harness,
        vec!["bank-a".to_owned()],
    )))
}

#[test]
fn a_bound_transport_names_its_credential_on_the_context() {
    let ctx = bound("ops-key")
        .resolve_ctx(&json!({"bank": "bank-a"}))
        .expect("ctx");
    assert_eq!(
        ctx.principal().map(clio_types::PrincipalLabel::as_str),
        Some("ops-key")
    );
    assert_eq!(ctx.actor, Actor::Harness);
    assert_eq!(ctx.bank.as_str(), "bank-a");
}

#[test]
fn a_local_transport_carries_no_credential_label() {
    let ctx = state()
        .resolve_ctx(&json!({"bank": "bank-a"}))
        .expect("ctx");
    assert_eq!(
        ctx.principal(),
        None,
        "a local process owner already holds the store, so there is nothing to name"
    );
}

#[test]
fn an_absent_declared_actor_on_a_bound_transport_uses_the_bound_one() {
    let ctx = bound("ops-key")
        .resolve_ctx(&json!({"bank": "bank-a", "actor": "harness"}))
        .expect("ctx");
    assert_eq!(ctx.actor, Actor::Harness);
    assert_eq!(
        ctx.principal().map(clio_types::PrincipalLabel::as_str),
        Some("ops-key")
    );
}

#[test]
fn a_bound_transport_refuses_a_declared_actor_it_does_not_name() {
    let err = bound("ops-key")
        .resolve_ctx(&json!({"bank": "bank-a", "actor": "system"}))
        .expect_err("actor conflict");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn a_bound_transport_refuses_a_bank_outside_its_list() {
    let err = bound("ops-key")
        .resolve_ctx(&json!({"bank": "bank-b"}))
        .expect_err("bank outside list");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn a_local_transport_still_takes_the_declared_actor() {
    let ctx = state()
        .resolve_ctx(&json!({"bank": "bank-a", "actor": "user"}))
        .expect("ctx");
    assert_eq!(ctx.actor, Actor::User);
    assert_eq!(ctx.principal(), None);
}

#[test]
fn the_shared_bank_needs_the_deployment_to_enable_it() {
    let err = state()
        .resolve_ctx(&json!({"bank": SHARED_BANK}))
        .expect_err("shared bank disabled");
    assert_eq!(err.code, ErrorCode::Forbidden);

    let enabled = McpState::open_with_effective(
        &McpOpenOptions {
            shared_bank_enabled: true,
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("state");
    assert_eq!(
        enabled
            .resolve_ctx(&json!({"bank": SHARED_BANK}))
            .expect("ctx")
            .bank
            .as_str(),
        SHARED_BANK
    );
}

#[test]
fn an_empty_resolved_bank_is_refused_on_both_paths() {
    assert_eq!(
        state()
            .resolve_ctx(&json!({"bank": ""}))
            .expect_err("empty bank")
            .code,
        ErrorCode::MissingBank
    );
    // On the bound path the credential's list refuses the empty bank first, so
    // the error names the refusal rather than the missing argument. Both are a
    // refusal and neither reaches a write.
    assert_eq!(
        bound("ops-key")
            .resolve_ctx(&json!({"bank": ""}))
            .expect_err("empty bank")
            .code,
        ErrorCode::Forbidden
    );
}

#[test]
fn utc_stamp_formats_a_known_second_and_now_utc_is_stamped() {
    // 1970-01-01T00:00:00Z and one second past a day boundary.
    assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
    assert_eq!(utc_stamp(86_400), "1970-01-02T00:00:00Z");
    assert_eq!(utc_stamp(86_401), "1970-01-02T00:00:01Z");
    // Shape only: the value comes from the host clock.
    assert!(now_utc().ends_with('Z'));
    assert_eq!(now_utc().len(), "1970-01-01T00:00:00Z".len());
}

#[test]
fn clean_batch_ids_do_not_repeat() {
    let s = state();
    assert_ne!(s.next_clean_batch_id(), s.next_clean_batch_id());
}

/// Store one item through the shared write tool and report its stored stamps.
///
/// The item travels the ordinary path — `dispatch` → `store` → the store's
/// insert — so what is asserted is the column a later text comparison reads,
/// not the resolver's return value.
fn stored_stamps(id: &str, timestamp: &str) -> (String, String) {
    let s = state();
    let args = json!({
        "item": {
            "id": id,
            "subject_id": format!("subj-{id}"),
            "gist": format!("prefers terse answers {id}"),
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers terse answers {id} (1 note since 2024-01-01)."),
        "bank": "bank-a",
        "actor": "agent",
        "timestamp": timestamp
    });
    let out = crate::write_tools::dispatch(&s, "store", &args);
    assert_eq!(out["ok"], json!(true), "store refused {timestamp}: {out}");
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
    let row = rows
        .into_iter()
        .find(|row| row.item_id == id)
        .expect("the item the write reported");
    (row.created_at, row.valid_from)
}

#[test]
fn a_fractional_timestamp_is_stored_as_the_second_it_names() {
    let (created_at, valid_from) = stored_stamps("itm-frac", "2026-10-12T08:30:01.500Z");
    assert_eq!(created_at, "2026-10-12T08:30:01Z");
    assert_eq!(valid_from, "2026-10-12T08:30:01Z");
    // The bound the fix exists for: a second-precision bound now keeps the row.
    assert!(
        created_at.as_str() <= "2026-10-12T08:30:01Z",
        "{created_at}"
    );
}

#[test]
fn an_offset_timestamp_is_stored_as_its_utc_instant() {
    let (created_at, valid_from) = stored_stamps("itm-offset", "2026-10-12T08:30:01+02:00");
    assert_eq!(created_at, "2026-10-12T06:30:01Z");
    assert_eq!(valid_from, "2026-10-12T06:30:01Z");
}

#[test]
fn a_canonical_timestamp_is_stored_unchanged() {
    let (created_at, valid_from) = stored_stamps("itm-plain", "2026-10-12T08:30:01Z");
    assert_eq!(created_at, "2026-10-12T08:30:01Z");
    assert_eq!(valid_from, "2026-10-12T08:30:01Z");
}

#[test]
fn a_timestamp_that_cannot_be_placed_is_refused_with_invalid_argument() {
    let s = state();
    let args = json!({
        "item": {
            "id": "itm-bad",
            "subject_id": "subj-itm-bad",
            "gist": "prefers terse answers itm-bad",
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": "The user prefers terse answers itm-bad (1 note since 2024-01-01).",
        "bank": "bank-a",
        "actor": "agent",
        // A month the year does not have: the store cannot place it, so the
        // write must refuse rather than store a literal string in a time column.
        "timestamp": "2026-13-01T00:00:00Z"
    });
    let out = crate::write_tools::dispatch(&s, "store", &args);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(
        out["code"],
        json!("invalid_argument"),
        "refusal must reuse the invalid-argument code: {out}"
    );
    assert_eq!(
        canonical_write_stamp("not a stamp")
            .expect_err("unplaceable")
            .code,
        ErrorCode::InvalidArgument
    );
}
