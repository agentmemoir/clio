// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync dead-letter quarantine helpers: the explicit
//! reason a refusal records and the outcome it answers with, the quarantine that
//! lands in the authorized bank rather than the refused one, and the mapped
//! reason a storage-shaped refusal gets.
//!
//! ## Owns
//! - Behavior coverage for `quarantine`, `dead_letter_with_reason`, and
//!   `dead_letter`
//!
//! ## Does not own
//! - The reason-code vocabulary (`protocol_tests`)
//! - The apply matrix that calls these helpers (`apply_matrix_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the quarantine row shape or its reason mapping changes;
//! they never change it.
//!
//! Keep this module focused on the dead-letter helper tests.

use super::{dead_letter, dead_letter_with_reason, quarantine};
use crate::apply::apply_tests::{fixture, upsert};
use crate::apply::{ApplyEngine, ApplyOutcome};
use crate::freshness::FreshnessPolicy;
use clio_store::{DeadLetterStatus, SqliteStore, SyncStore};
use clio_types::{AmError, ErrorCode};

/// The engine a quarantine runs under.
fn engine_of(store: &SqliteStore) -> ApplyEngine<'_> {
    ApplyEngine {
        store,
        sync: store,
        device_id: "node-1".into(),
        kms: None,
        policy: FreshnessPolicy::default(),
    }
}

/// The open dead-letter rows recorded for a bank.
fn open(store: &SqliteStore, bank: &str) -> Vec<clio_store::SyncDeadLetterRow> {
    store
        .dead_letter_list(bank, DeadLetterStatus::Open, 10)
        .expect("dead letters")
}

/// An explicit reason is recorded verbatim together with its detail, and the
/// refused mutation answers `DeadLettered` with that same reason.
#[test]
fn an_explicit_reason_is_recorded_verbatim_and_answers_dead_lettered() {
    let (store, _kms) = fixture();
    let m = upsert(
        "m-explicit",
        "b-verbatim",
        "2026-01-01T00:00:00Z",
        "dev-a",
        "g",
    );
    let outcome = dead_letter_with_reason(
        &engine_of(&store),
        &m,
        "{\"envelope\":true}",
        "unsupported_op",
        "this node cannot replay the op",
    )
    .expect("dead letter");
    assert_eq!(
        outcome,
        ApplyOutcome::DeadLettered {
            reason_code: "unsupported_op".into()
        }
    );
    let rows = open(&store, "b-verbatim");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason_code, "unsupported_op");
    assert_eq!(rows[0].detail, "this node cannot replay the op");
    assert_eq!(rows[0].envelope_json, "{\"envelope\":true}");
    assert_eq!(rows[0].sync_mutation_id, "m-explicit");
    assert_eq!(rows[0].bank_id, "b-verbatim");
}

/// The operator quarantine keeps its own reason and never degrades to the
/// mapped `malformed` answer, and its row lands in the bank the caller names.
#[test]
fn a_quarantine_keeps_its_reason_and_lands_in_the_named_bank() {
    let (store, _kms) = fixture();
    let m = upsert("m-held", "b-refused", "2026-01-01T00:00:00Z", "dev-a", "g");
    let outcome = quarantine(
        &engine_of(&store),
        "b-authorized",
        &m,
        "{}",
        crate::protocol::REASON_UNAUTHORIZED_BANK,
        "the credential cannot reach b-refused",
    )
    .expect("quarantine");
    assert_eq!(
        outcome,
        ApplyOutcome::DeadLettered {
            reason_code: crate::protocol::REASON_UNAUTHORIZED_BANK.into()
        },
        "the quarantine reason is the caller's, not the mapped one"
    );
    assert!(
        open(&store, "b-refused").is_empty(),
        "a refusal writes nothing into the bank it refused"
    );
    let rows = open(&store, "b-authorized");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason_code, "unauthorized_bank");
    assert_eq!(rows[0].sync_mutation_id, "m-held");
    assert_eq!(rows[0].bank_id, "b-authorized");
}

/// The mapped helper reads the reason out of the error it is given: an erased
/// subject refusal maps to `erased_subject` and carries the error message as its
/// detail.
#[test]
fn a_mapped_reason_follows_the_error_and_keeps_its_message() {
    let (store, _kms) = fixture();
    let m = upsert("m-mapped", "b-mapped", "2026-01-01T00:00:00Z", "dev-a", "g");
    let err = AmError::new(ErrorCode::ErasedSubject, "subject is erased");
    let outcome = dead_letter(&engine_of(&store), &m, "{}", &err).expect("dead letter");
    assert_eq!(
        outcome,
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
    let rows = open(&store, "b-mapped");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason_code, "erased_subject");
    assert_eq!(rows[0].detail, "subject is erased");
}

/// Re-quarantining the same mutation id keeps one row, so a retried refusal does
/// not grow the operator's list.
#[test]
fn a_repeated_refusal_keeps_one_row() {
    let (store, _kms) = fixture();
    let m = upsert("m-twice", "b-repeat", "2026-01-01T00:00:00Z", "dev-a", "g");
    for _ in 0..2 {
        quarantine(
            &engine_of(&store),
            "b-repeat",
            &m,
            "{}",
            crate::protocol::REASON_MALFORMED,
            "malformed",
        )
        .expect("quarantine");
    }
    assert_eq!(open(&store, "b-repeat").len(), 1);
    assert_eq!(
        store
            .dead_letter_count("b-repeat", DeadLetterStatus::Open)
            .expect("count"),
        1
    );
}
