// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the sync dead-letter quarantine
//! helpers directly: the operator-forced row, the reason-code mapped row, the
//! freshness refusal record, and the stale-refusal counter with its last line.
//!
//! ## Owns
//! - The dead-letter row each helper opens: bank, reason code, and detail
//! - The `stale_refused` counter and the `__status` last-refusal line
//!
//! ## Does not own
//! - The freshness decision itself (`apply_freshness_tests`)
//! - The push and pull routes that call these helpers (`server_push_stale_tests`,
//!   `client_freshness_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the quarantine row shape or the refusal record changes;
//! they never change it.
//!
//! Keep this module focused on the dead-letter helpers.

use crate::apply::stale_refused;
use crate::apply::{ApplyOutcome, dead_letter, last_stale_refusal, quarantine, quarantine_stale};
use crate::apply_fixtures::{fixture, upsert};
use crate::protocol::{REASON_MALFORMED_STAMP, REASON_STALE_ENVELOPE};
use clio_store::{DeadLetterStatus, SyncStore as _};
use clio_types::{AmError, ErrorCode};

/// One bank for every case here, and one fixed stamp: no case reads the clock.
const BANK: &str = "b-dlq";
const STAMP: &str = "2026-01-01T00:00:00Z";
const NOW: i64 = 1_767_225_600;

#[test]
fn quarantine_opens_an_operator_facing_dead_letter_row() {
    let (store, _kms) = fixture();
    let m = upsert("m-q", BANK, STAMP, "dev-a", "g");
    let engine = crate::apply::apply_freshness::test_engine(
        store.as_ref(),
        store.as_ref(),
        "node-1",
        None,
        &m.transaction_time,
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    let out = quarantine(&engine, BANK, &m, &envelope, "malformed", "no key").expect("quarantine");
    assert_eq!(
        out,
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    assert_eq!(
        store
            .dead_letter_count(BANK, DeadLetterStatus::Open)
            .expect("count"),
        1
    );
}

#[test]
fn dead_letter_maps_the_error_code_to_a_reason() {
    let (store, _kms) = fixture();
    let m = upsert("m-dl", BANK, STAMP, "dev-a", "g");
    let engine = crate::apply::apply_freshness::test_engine(
        store.as_ref(),
        store.as_ref(),
        "node-1",
        None,
        &m.transaction_time,
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    let err = AmError::new(ErrorCode::NotImplemented, "op not supported");
    assert_eq!(
        dead_letter(&engine, &m, &envelope, &err).expect("dead letter"),
        ApplyOutcome::DeadLettered {
            reason_code: "unsupported_op".into()
        }
    );
    let rows = store
        .dead_letter_list(BANK, DeadLetterStatus::Open, 10)
        .expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason_code, "unsupported_op");
    assert_eq!(rows[0].detail, "op not supported");
    assert_eq!(rows[0].bank_id, BANK);
}

#[test]
fn a_placeable_stamp_is_recorded_as_a_stale_envelope_refusal() {
    let (store, _kms) = fixture();
    let m = upsert("m-stale", BANK, STAMP, "dev-a", "g");
    let engine = crate::apply::apply_freshness::test_engine_at(
        store.as_ref(),
        store.as_ref(),
        "node-1",
        None,
        NOW,
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert_eq!(
        quarantine_stale(&engine, BANK, &m, &envelope).expect("quarantine"),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_STALE_ENVELOPE.into()
        }
    );
    let rows = store
        .dead_letter_list(BANK, DeadLetterStatus::Open, 10)
        .expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason_code, REASON_STALE_ENVELOPE);
    assert_eq!(rows[0].envelope_json, envelope);
    assert_eq!(stale_refused(store.as_ref(), BANK).expect("count"), 1);
    let line = last_stale_refusal(store.as_ref())
        .expect("status read")
        .expect("a refusal was recorded");
    assert!(line.contains(REASON_STALE_ENVELOPE), "{line}");
    assert!(line.contains("m-stale"), "{line}");
    assert!(line.contains(STAMP), "{line}");
}

#[test]
fn an_unplaceable_stamp_is_recorded_as_a_malformed_refusal() {
    let (store, _kms) = fixture();
    let mut m = upsert("m-bad", BANK, STAMP, "dev-a", "g");
    m.transaction_time = String::new();
    let engine = crate::apply::apply_freshness::test_engine_at(
        store.as_ref(),
        store.as_ref(),
        "node-1",
        None,
        NOW,
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    assert_eq!(
        quarantine_stale(&engine, BANK, &m, &envelope).expect("quarantine"),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_MALFORMED_STAMP.into()
        }
    );
    let line = last_stale_refusal(store.as_ref())
        .expect("status read")
        .expect("a refusal was recorded");
    assert!(line.contains(REASON_MALFORMED_STAMP), "{line}");
}

#[test]
fn the_refusal_count_adds_one_to_the_existing_value() {
    let (store, _kms) = fixture();
    let m = upsert("m-stale", BANK, STAMP, "dev-a", "g");
    store
        .cursor_set(BANK, "stale_refused", "4")
        .expect("seed count");
    let engine = crate::apply::apply_freshness::test_engine_at(
        store.as_ref(),
        store.as_ref(),
        "node-1",
        None,
        NOW,
    );
    let envelope = serde_json::to_string(&m).expect("envelope");
    quarantine_stale(&engine, BANK, &m, &envelope).expect("quarantine");
    assert_eq!(stale_refused(store.as_ref(), BANK).expect("count"), 5);
}

#[test]
fn a_bank_that_refused_nothing_reports_zero_and_no_status_line() {
    let (store, _kms) = fixture();
    assert_eq!(stale_refused(store.as_ref(), BANK).expect("count"), 0);
    assert_eq!(
        last_stale_refusal(store.as_ref()).expect("status read"),
        None
    );
    store
        .cursor_set(BANK, "stale_refused", "not-a-number")
        .expect("seed count");
    assert_eq!(
        stale_refused(store.as_ref(), BANK).expect("count"),
        0,
        "an unreadable count reads as zero rather than an error"
    );
}
