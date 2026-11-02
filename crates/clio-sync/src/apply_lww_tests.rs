// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the last-writer-wins helper the apply matrix runs against a
//! locally held row: the tolerant admission-score read, the later-stamp,
//! equal-stamp-score, and equal-score-device decision branches, and the conflict
//! row a lost comparison records.
//!
//! ## Owns
//! - Behavior coverage for `admission_score` and `lww_loser`
//!
//! ## Does not own
//! - The ordering rule itself (`protocol_tests`)
//! - The apply matrix that calls this helper (`apply_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the comparison helper or its conflict row changes; they
//! never change it.
//!
//! Keep this module focused on the LWW helper tests.

use super::{admission_score, lww_loser};
use crate::apply::ApplyEngine;
use crate::apply::apply_tests::{fixture, receipt, upsert};
use crate::freshness::FreshnessPolicy;
use clio_store::{SqliteStore, SyncStore};
use serde_json::json;

/// The engine the comparison runs under: this node's device id is the existing
/// side of a full tie.
fn engine_of(store: &SqliteStore) -> ApplyEngine<'_> {
    ApplyEngine {
        store,
        sync: store,
        device_id: "node-1".into(),
        kms: None,
        policy: FreshnessPolicy::default(),
    }
}

/// The admission score is read only from a numeric `score`; a missing key and a
/// value that is not a number both answer `None`.
#[test]
fn admission_score_reads_a_numeric_score_and_ignores_everything_else() {
    assert_eq!(admission_score(&json!({ "score": 0.75 })), Some(0.75));
    assert_eq!(admission_score(&json!({ "score": 2 })), Some(2.0));
    assert_eq!(admission_score(&json!({ "admitted": true })), None);
    assert_eq!(admission_score(&json!({ "score": "0.5" })), None);
    assert_eq!(admission_score(&json!({})), None);
}

/// A later incoming stamp wins outright, even against a higher stored score, and
/// nothing is recorded as a conflict.
#[test]
fn a_later_stamp_wins_without_a_conflict_row() {
    let (store, _kms) = fixture();
    let m = upsert("m-new", "b-lww", "2026-01-02T00:00:00Z", "peer-a", "newer");
    let loser =
        lww_loser(&engine_of(&store), &m, "2026-01-01T00:00:00Z", Some(0.9)).expect("compare runs");
    assert!(!loser, "a later stamp is not a loser");
    assert_eq!(store.conflict_count("b-lww").expect("count"), 0);
}

/// An earlier incoming stamp loses and the conflict row names both sides.
#[test]
fn an_earlier_stamp_loses_and_the_conflict_row_is_recorded() {
    let (store, _kms) = fixture();
    let m = upsert(
        "m-old",
        "b-lww",
        "2026-01-01T00:00:00Z",
        "dev-late",
        "older",
    );
    let loser =
        lww_loser(&engine_of(&store), &m, "2026-01-02T00:00:00Z", Some(0.1)).expect("compare runs");
    assert!(loser, "an earlier stamp is a loser");
    let conflicts = store.conflict_list("b-lww", 5).expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    let row = &conflicts[0];
    assert_eq!(row.winner_mutation_id, "local:i-1");
    assert_eq!(row.loser_mutation_id, "m-old");
    assert_eq!(row.winner_transaction_time, "2026-01-02T00:00:00Z");
    assert_eq!(row.loser_transaction_time, "2026-01-01T00:00:00Z");
    assert_eq!(row.winner_device_id, "node-1");
    assert_eq!(row.loser_device_id, "dev-late");
}

/// At an equal stamp the score decides: a lower incoming score loses and leaves
/// a conflict row, a higher one wins and leaves none.
#[test]
fn an_equal_stamp_falls_to_the_admission_score() {
    let (store, _kms) = fixture();
    let stamp = "2026-01-01T00:00:00Z";
    let weak = receipt(&upsert("m-weak", "b-score", stamp, "peer-a", "weak"), 0.4);
    assert!(
        lww_loser(&engine_of(&store), &weak, stamp, Some(0.5)).expect("compare runs"),
        "the lower score loses"
    );
    let strong = receipt(
        &upsert("m-strong", "b-score", stamp, "peer-a", "strong"),
        0.9,
    );
    assert!(
        !lww_loser(&engine_of(&store), &strong, stamp, Some(0.5)).expect("compare runs"),
        "the higher score wins"
    );
    assert_eq!(store.conflict_count("b-score").expect("count"), 1);
    let conflicts = store.conflict_list("b-score", 5).expect("conflicts");
    assert_eq!(conflicts[0].loser_mutation_id, "m-weak");
}

/// At an equal stamp and score the device id breaks the tie, compared against
/// this node's own device id.
#[test]
fn an_equal_score_falls_to_the_device_id() {
    let (store, _kms) = fixture();
    let stamp = "2026-01-01T00:00:00Z";
    let below = receipt(&upsert("m-below", "b-dev", stamp, "dev-a", "below"), 0.5);
    assert!(
        lww_loser(&engine_of(&store), &below, stamp, Some(0.5)).expect("compare runs"),
        "a device id below `node-1` loses"
    );
    let above = receipt(&upsert("m-above", "b-dev", stamp, "peer-z", "above"), 0.5);
    assert!(
        !lww_loser(&engine_of(&store), &above, stamp, Some(0.5)).expect("compare runs"),
        "a device id above `node-1` wins"
    );
    let conflicts = store.conflict_list("b-dev", 5).expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].loser_mutation_id, "m-below");
    assert_eq!(conflicts[0].loser_device_id, "dev-a");
}

/// An incoming row with no receipt is compared at score `0.0`, the documented
/// absent-score fallback, so it loses an equal-stamp comparison against a
/// stored positive score.
#[test]
fn an_absent_receipt_is_compared_at_the_documented_zero() {
    let (store, _kms) = fixture();
    let stamp = "2026-01-01T00:00:00Z";
    let bare = upsert("m-bare", "b-zero", stamp, "peer-z", "bare");
    assert!(
        lww_loser(&engine_of(&store), &bare, stamp, Some(0.01)).expect("compare runs"),
        "the absent score is zero and loses to a positive one"
    );
}
