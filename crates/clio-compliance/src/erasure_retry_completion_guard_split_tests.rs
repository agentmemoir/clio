// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the privileged-actor refusal coverage of the
//! confirmed-erasure retry entry: an agent or a user actor is refused before
//! any durable intent, leaving no action row, no journal denial, no key loss,
//! no tombstone, and the subject's data readable.
//!
//! ## Owns
//! - The unprivileged-actor refusal assertions over the retry entry
//! - The shared refusal probe both actor tests drive
//!
//! ## Does not own
//! - The flow (`erasure_retry.rs`), the reconcile (`erasure_reconcile.rs`),
//!   and the shared fixtures (`erasure_retry_tests.rs`,
//!   `subject_request_store_wiring_tests.rs`)
//! - The remaining completion-guard assertions
//!   (`erasure_retry_completion_guard_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when an unprivileged actor reaches the durable intent or
//! the effect instead of being refused before any write, or when the refusal
//! leaves a durable trace behind.
//!
//! Test-only. Never compiled into production builds.

use super::super::erasure_retry_tests::{live_set, persisted, tombstone_count};
use super::super::subject_request_store_wiring_tests::wiring_item;
use crate::erase::EraseVecSink;
use crate::erasure_retry::erase_confirmed_request;
use clio_store::{ComplianceStore, LocalEventStore, Store};
use clio_types::{Actor, Bank, ErrorCode, OpContext};

/// Drive the retry entry with `actor` and pin that its refusal precedes every
/// durable write: no intent or action row, no journal denial, no key loss, no
/// tombstone, and the subject's data stays readable.
fn assert_unprivileged_refusal(actor: Actor) {
    let (attempt, store) = persisted();
    let item = wiring_item(&attempt.bank, &attempt.subject);
    store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    let ctx = OpContext::new(Bank::new(attempt.bank.clone()), actor);
    let sink = EraseVecSink::default();
    let args = attempt.args(&store, &ctx, &sink);
    let live = live_set(&attempt, &store);
    let err = erase_confirmed_request(&store, &store, &store, &attempt.request_id, &args, &live)
        .expect_err("an unprivileged actor must refuse");
    assert_eq!(
        err.code,
        ErrorCode::Forbidden,
        "{}: {}",
        actor.as_str(),
        err.message
    );
    assert!(
        store.list_events().expect("events").is_empty(),
        "{} writes no intent row",
        actor.as_str()
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "{} leaves no pending action",
        actor.as_str()
    );
    assert!(
        !attempt.journal.exists(),
        "{} writes no denial",
        actor.as_str()
    );
    assert!(
        !store.subject_dek_destroyed(&attempt.subject).expect("key"),
        "{} loses no key",
        actor.as_str()
    );
    assert_eq!(
        tombstone_count(&attempt, &store),
        0,
        "{} runs no store effect",
        actor.as_str()
    );
    assert!(
        store
            .get_memory_item(&attempt.bank, &item.id)
            .expect("readable")
            .is_some(),
        "{} leaves the subject's data",
        actor.as_str()
    );
}

/// An agent actor is refused before the durable intent exists.
#[test]
fn an_agent_actor_refuses_before_the_durable_intent() {
    assert_unprivileged_refusal(Actor::Agent);
}

/// A user actor is refused before the durable intent exists.
#[test]
fn a_user_actor_refuses_before_the_durable_intent() {
    assert_unprivileged_refusal(Actor::User);
}
