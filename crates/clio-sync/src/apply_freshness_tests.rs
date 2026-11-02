// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the envelope-order guards of the sync apply engine: the
//! future-skew refusal, the per-device high-water refusal, the accepted equal
//! stamp, and the way a refusal answers through the dead-letter path so the
//! pull cursor freezes instead of advancing.
//!
//! ## Owns
//! - Behavior coverage for `apply_freshness`: which envelopes are refused and
//!   which are applied, with an injected `now`
//!
//! ## Does not own
//! - The pure bounds themselves (`freshness_tests`)
//! - Transport and cursor mechanics (`client_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the refusal rule changes; they never change it.
//!
//! Keep this module focused on envelope-order guard tests.

use crate::apply::apply_freshness::guard;
use crate::apply::apply_tests::{apply, env, fixture, item_value, upsert};
use crate::apply::{ApplyEngine, ApplyOutcome};
use crate::freshness::FreshnessPolicy;
use crate::protocol::{ENTITY_KIND_ITEM, ENTITY_KIND_PERSONA, OP_UPSERT, REASON_STALE_ENVELOPE};
use clio_store::{DeadLetterStatus, SqliteStore, Store, SyncStore};

/// One injected `now`, so no test reads the wall clock.
const NOW: &str = "2026-01-01T00:00:00Z";

/// Run the guard with an injected `now` and report whether it refused.
fn guarded_at(store: &SqliteStore, m: &crate::protocol::SyncMutation) -> Option<ApplyOutcome> {
    let engine = ApplyEngine {
        store,
        sync: store,
        device_id: "node-1".into(),
        kms: None,
        policy: FreshnessPolicy::default(),
    };
    let envelope = serde_json::to_string(m).expect("serialize envelope");
    guard(&engine, m, &envelope, NOW)
        .expect("guard runs")
        .map(|result| result.outcome)
}

/// The open dead-letter reason codes recorded for a bank.
fn open_reasons(store: &SqliteStore, bank: &str) -> Vec<String> {
    store
        .dead_letter_list(bank, DeadLetterStatus::Open, 10)
        .expect("list dead letters")
        .into_iter()
        .map(|row| row.reason_code)
        .collect()
}

#[test]
fn t25_01_envelope_ahead_of_now_by_61_seconds_is_dead_lettered() {
    let (store, _kms) = fixture();
    let m = upsert("m-future", "b-future", "2026-01-01T00:01:01Z", "dev-a", "g");
    assert_eq!(
        guarded_at(&store, &m),
        Some(ApplyOutcome::DeadLettered {
            reason_code: REASON_STALE_ENVELOPE.into()
        })
    );
    assert_eq!(
        open_reasons(&store, "b-future"),
        vec![REASON_STALE_ENVELOPE]
    );
    assert!(
        store
            .get_memory_item("b-future", "i-1")
            .expect("read item")
            .is_none()
    );
    assert!(
        store
            .journal_get("b-future", "m-future")
            .expect("read journal")
            .is_none()
    );
}

#[test]
fn t25_02_envelope_ahead_of_now_by_59_seconds_applies() {
    let (store, _kms) = fixture();
    let m = upsert("m-lead", "b-lead", "2026-01-01T00:00:59Z", "dev-a", "g");
    assert_eq!(guarded_at(&store, &m), None);
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
}

#[test]
fn t25_03_envelope_one_year_old_applies_because_age_is_not_the_defence() {
    let (store, _kms) = fixture();
    let m = upsert("m-old", "b-old", "2025-01-01T00:00:00Z", "dev-a", "g");
    assert_eq!(guarded_at(&store, &m), None);
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
}

#[test]
fn t25_04_envelope_older_than_the_device_mark_is_dead_lettered() {
    let (store, _kms) = fixture();
    let bank = "b-mark";
    let first = upsert("m-first", bank, "2026-01-01T00:00:02Z", "dev-a", "g1");
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    let older = upsert("m-older", bank, "2026-01-01T00:00:01Z", "dev-a", "g2");
    assert_eq!(
        apply(&store, None, &older),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_STALE_ENVELOPE.into()
        }
    );
    assert_eq!(open_reasons(&store, bank), vec![REASON_STALE_ENVELOPE]);
}

#[test]
fn t25_05_envelope_above_the_device_mark_applies_and_raises_the_mark() {
    let (store, _kms) = fixture();
    let bank = "b-raise";
    let first = upsert("m-low", bank, "2026-01-01T00:00:01Z", "dev-a", "g1");
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    assert_eq!(
        store
            .device_high_water_get(bank, "dev-a", ENTITY_KIND_ITEM)
            .expect("read mark")
            .as_deref(),
        Some("2026-01-01T00:00:01Z")
    );
    let higher = upsert("m-high", bank, "2026-01-01T00:00:05Z", "dev-a", "g2");
    assert_eq!(apply(&store, None, &higher), ApplyOutcome::Applied);
    assert_eq!(
        store
            .device_high_water_get(bank, "dev-a", ENTITY_KIND_ITEM)
            .expect("read mark")
            .as_deref(),
        Some("2026-01-01T00:00:05Z")
    );
}

#[test]
fn t25_06_replaying_an_applied_id_still_answers_duplicate() {
    let (store, _kms) = fixture();
    let bank = "b-replay";
    let m = upsert("m-keep", bank, "2026-01-01T00:00:01Z", "dev-a", "g1");
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    // The stamp is now at the device mark; the journal check answers first.
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Duplicate);
    assert_eq!(open_reasons(&store, bank), Vec::<String>::new());
}

#[test]
fn t25_07_an_acked_skip_still_answers_duplicate_and_is_not_re_frozen() {
    let (store, _kms) = fixture();
    let bank = "b-acked";
    let first = upsert("m-base", bank, "2026-01-01T00:00:02Z", "dev-a", "g1");
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    let older = upsert("m-poison", bank, "2026-01-01T00:00:01Z", "dev-a", "g2");
    assert_eq!(
        apply(&store, None, &older),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_STALE_ENVELOPE.into()
        }
    );
    assert!(
        store
            .dead_letter_ack_skip("m-poison", "2026-01-01T00:00:03Z")
            .expect("ack skip")
    );
    assert_eq!(apply(&store, None, &older), ApplyOutcome::Duplicate);
    assert_eq!(open_reasons(&store, bank), Vec::<String>::new());
}

#[test]
fn t25_09_a_mark_from_one_kind_does_not_refuse_another_kind() {
    let (store, _kms) = fixture();
    let bank = "b-kinds";
    // The persona stamp is epoch milliseconds while the item stamp is ISO-8601
    // seconds, and those two encodings compare wrongly as text. Each kind keeps
    // its own mark, so an older-looking stamp in a different encoding is not a
    // replay.
    let persona = env(
        "m-persona",
        bank,
        ENTITY_KIND_PERSONA,
        "style",
        OP_UPSERT,
        "1793597865118",
        "dev-a",
        serde_json::json!({
            "record": "stable",
            "entry": {
                "bank_id": bank,
                "key": "style",
                "value": "terse",
                "confidence": null,
                "source": "explicit",
                "source_ref": "ev",
                "context": null,
                "admission_score": 0.5,
                "as_of": "2026-01-01",
                "valid_until": null
            }
        }),
    );
    assert_eq!(apply(&store, None, &persona), ApplyOutcome::Applied);
    let item = upsert("m-item", bank, "2026-01-01T00:00:01Z", "dev-a", "g1");
    assert_eq!(apply(&store, None, &item), ApplyOutcome::Applied);
    assert_eq!(open_reasons(&store, bank), Vec::<String>::new());
}

#[test]
fn t25_08_a_second_row_at_the_same_stamp_from_one_device_applies() {
    let (store, _kms) = fixture();
    let bank = "b-equal";
    let first = upsert("m-one", bank, "2026-01-01T00:00:01Z", "dev-a", "g1");
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    // `transaction_time` has second granularity, so a batch packaged in one
    // pass carries one stamp across several rows: an equal stamp is not a
    // replay, and refusing it would dead-letter real traffic.
    let same_stamp = env(
        "m-two",
        bank,
        ENTITY_KIND_ITEM,
        "i-2",
        OP_UPSERT,
        "2026-01-01T00:00:01Z",
        "dev-a",
        item_value(bank, "i-2", "s1", "2026-01-01T00:00:01Z", "g2"),
    );
    assert_eq!(apply(&store, None, &same_stamp), ApplyOutcome::Applied);
    assert_eq!(open_reasons(&store, bank), Vec::<String>::new());
    assert_eq!(
        store
            .device_high_water_get(bank, "dev-a", ENTITY_KIND_ITEM)
            .expect("read mark")
            .as_deref(),
        Some("2026-01-01T00:00:01Z")
    );
}
