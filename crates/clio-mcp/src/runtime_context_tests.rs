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
    McpState::open(&McpOpenOptions::default()).expect("state")
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

    let enabled = McpState::open(&McpOpenOptions {
        shared_bank_enabled: true,
        ..McpOpenOptions::default()
    })
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
