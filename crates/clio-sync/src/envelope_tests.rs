// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the canonical encoding of the sync execution envelope:
//! that the same envelope always encodes to the same bytes, and that every
//! bound field changes them.
//!
//! ## Owns
//! - Unit coverage for `envelope` (`ExecutionView::aad`)
//! - The injectivity of the encoding: distinct envelopes, distinct bytes
//!
//! ## Does not own
//! - Seal/open behaviour (`crypto_tests`)
//! - What apply does with a refused row (`client_*_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the bound field list or the encoding layout changes;
//! they never change it.
//!
//! Keep this module focused on envelope encoding tests.

use crate::protocol::{SyncContent, SyncMutation};
use serde_json::json;

/// One single-field change to an envelope.
type FieldChange = (&'static str, fn(&mut SyncMutation));

/// One envelope with every field set, so each field can be varied alone.
fn base() -> SyncMutation {
    SyncMutation {
        sync_mutation_id: "m-1".into(),
        bank: "bank-a".into(),
        entity_kind: "item".into(),
        entity_id: "i-1".into(),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content: SyncContent::Plain(json!({})),
        content_hash: "hash-1".into(),
        admission_receipt: Some(json!({ "admitted": true })),
    }
}

#[test]
fn the_same_envelope_encodes_to_the_same_bytes() {
    let one = base();
    let two = base();
    assert_eq!(one.execution_view().aad(), two.execution_view().aad());
    assert_eq!(one.execution_view().aad(), one.execution_view().aad());
}

#[test]
fn every_bound_field_changes_the_bytes() {
    let reference = base().execution_view().aad();
    let mutations: [FieldChange; 9] = [
        ("sync_mutation_id", |m| m.sync_mutation_id = "m-2".into()),
        ("bank", |m| m.bank = "bank-b".into()),
        ("entity_kind", |m| m.entity_kind = "task".into()),
        ("entity_id", |m| m.entity_id = "i-2".into()),
        ("op", |m| m.op = "discard".into()),
        ("transaction_time", |m| {
            m.transaction_time = "2026-01-02T00:00:00Z".into();
        }),
        ("device_id", |m| m.device_id = "dev-b".into()),
        ("content_hash", |m| m.content_hash = "hash-2".into()),
        ("admission_receipt", |m| m.admission_receipt = None),
    ];
    for (field, change) in mutations {
        let mut mutation = base();
        change(&mut mutation);
        assert_ne!(
            mutation.execution_view().aad(),
            reference,
            "the encoding must change when {field} changes"
        );
    }
}

#[test]
fn the_sealed_content_is_not_part_of_the_envelope() {
    let mut changed = base();
    changed.content = SyncContent::Plain(json!({ "gist": "different" }));
    assert_eq!(
        changed.execution_view().aad(),
        base().execution_view().aad()
    );
}

/// Values whose text a concatenating encoding would let one field absorb into
/// another: every delimiter of the envelope's own syntax, the escape character,
/// a newline, a control character, the empty string, and a value that spells a
/// whole neighbouring field.
const HOSTILE_VALUES: [&str; 9] = [
    "",
    "\"",
    "\\",
    "{}",
    ",",
    ":",
    "a\nb",
    "a\u{7}b",
    "\"},{\"bank\":\"bank-b",
];

#[test]
fn distinct_field_values_never_share_the_same_bytes() {
    let mut seen: Vec<(&str, Vec<u8>)> = Vec::new();
    for value in HOSTILE_VALUES {
        let mut mutation = base();
        mutation.entity_id = value.into();
        let aad = mutation.execution_view().aad();
        for (other, bytes) in &seen {
            assert_ne!(
                &aad, bytes,
                "the entity_id values {other:?} and {value:?} encoded to the same bytes"
            );
        }
        // Stability: the same value still encodes to the same bytes.
        assert_eq!(
            aad,
            mutation.execution_view().aad(),
            "the value {value:?} encodes unstably"
        );
        seen.push((value, aad));
    }
    // No field may absorb text moved into it from its neighbour.
    let mut left = base();
    left.entity_id = "ab".into();
    left.device_id = "c".into();
    let mut right = base();
    right.entity_id = "a".into();
    right.device_id = "bc".into();
    assert_ne!(
        left.execution_view().aad(),
        right.execution_view().aad(),
        "fields whose text concatenates equally must not collide"
    );
}

#[test]
fn a_present_receipt_and_an_absent_one_encode_differently() {
    let mut absent = base();
    absent.admission_receipt = None;
    let mut present = base();
    present.admission_receipt = Some(json!({}));
    let mut scored = base();
    scored.admission_receipt = Some(json!({ "score": 0.9 }));
    assert_ne!(
        absent.execution_view().aad(),
        present.execution_view().aad(),
        "an absent receipt must not encode as a present one"
    );
    assert_ne!(
        present.execution_view().aad(),
        scored.execution_view().aad(),
        "the receipt's own content is bound"
    );
    // Stability: an equal receipt encodes to equal bytes.
    assert_eq!(
        present.execution_view().aad(),
        present.execution_view().aad(),
        "the same present receipt encodes unstably"
    );
}
