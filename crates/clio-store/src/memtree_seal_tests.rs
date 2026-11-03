// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the sealed MemTree summary-gist
//! field: sealing it for storage, opening it back, and the binding between the
//! two.
//!
//! ## Owns
//! - Round-trip and `None` handling for `seal_gist` and `open_gist`
//! - The binding that rejects a tampered body, another bank, or another node
//! - The tolerant open, and the intolerant seal, after a bank's DEK is destroyed
//!
//! ## Does not own
//! - Envelope shape and DEK provider behaviour (see `dek`, `dek_tests`)
//! - MemTree row persistence and SQL (see `memtree_model_persist_tests`)
//!
//! ## Boundary
//!
//! Changes to what a sealed gist is bound to should normally require changes
//! here. Changes to the envelope shape should not require changes here.
//!
//! Keep this module focused on the summary-gist seal and open.

use super::{open_gist, seal_gist};
use crate::dek::{CiphertextEnvelope, DekProvider, LocalDevKms};
use crate::memtree_model::MemTreeNodeRecord;

const BANK: &str = "bank-seal";
const NODE: &str = "node-1";
const GIST: &str = "we discussed the move to Lisbon and the visa deadline";

fn node(bank: &str, id: &str, gist: Option<&str>) -> MemTreeNodeRecord {
    MemTreeNodeRecord {
        id: id.into(),
        bank_id: bank.into(),
        parent_id: None,
        item_id: Some("item-1".into()),
        depth: 0,
        interval_start: "100".into(),
        interval_end: "100".into(),
        child_ids: vec![],
        summary_gist: gist.map(str::to_owned),
        dirty: true,
        generation: 1,
        refreshing: false,
        task_id: None,
        last_error: None,
        created_at: "100".into(),
        updated_at: "100".into(),
    }
}

/// Seal `gist` for `bank`/`id` and hand back the storage text.
fn sealed(kms: &LocalDevKms, bank: &str, id: &str, gist: &str) -> String {
    seal_gist(kms, &node(bank, id, Some(gist)))
        .expect("seal")
        .expect("a present gist seals to some text")
}

/// Encode `envelope` with one middle character of its ciphertext changed, so
/// the body still parses but no longer authenticates.
fn tampered(envelope: &CiphertextEnvelope) -> String {
    let mut ciphertext_b64 = envelope.ciphertext_b64.clone();
    let middle = ciphertext_b64.len() / 2;
    let replacement = if ciphertext_b64.as_bytes()[middle] == b'A' {
        'B'
    } else {
        'A'
    };
    ciphertext_b64.replace_range(middle..=middle, &replacement.to_string());
    CiphertextEnvelope {
        ciphertext_b64,
        ..envelope.clone()
    }
    .to_storage_string()
}

#[test]
fn seal_gist_without_a_gist_writes_nothing() {
    let kms = LocalDevKms::new();
    let stored = seal_gist(&kms, &node(BANK, NODE, None)).expect("seal");
    assert_eq!(stored, None);
}

#[test]
fn seal_gist_hides_the_plaintext() {
    let kms = LocalDevKms::new();
    let stored = sealed(&kms, BANK, NODE, GIST);

    assert!(
        !stored.contains(GIST),
        "the plaintext gist must not survive into the stored text: {stored}"
    );
    // The stored form is the usual envelope, bound to the bank as its subject.
    let envelope = CiphertextEnvelope::from_storage_string(&stored).expect("envelope");
    assert_eq!(envelope.subject_id, BANK);
    assert_ne!(envelope.ciphertext_b64, "");
}

#[test]
fn open_gist_round_trips_the_sealed_gist() {
    let kms = LocalDevKms::new();
    let stored = sealed(&kms, BANK, NODE, GIST);

    assert_eq!(
        open_gist(&kms, BANK, NODE, Some(&stored)).as_deref(),
        Some(GIST)
    );
}

#[test]
fn open_gist_without_a_stored_value_returns_none() {
    let kms = LocalDevKms::new();

    assert_eq!(open_gist(&kms, BANK, NODE, None), None);
    assert_eq!(
        open_gist(&kms, BANK, NODE, Some("")),
        None,
        "an empty column means no gist, not a failed read"
    );
}

#[test]
fn open_gist_with_tampered_text_returns_none() {
    let kms = LocalDevKms::new();
    let stored = sealed(&kms, BANK, NODE, GIST);
    let envelope = CiphertextEnvelope::from_storage_string(&stored).expect("envelope");

    // Not an envelope at all, and an envelope missing its body.
    assert_eq!(open_gist(&kms, BANK, NODE, Some("not an envelope")), None);
    assert_eq!(
        open_gist(&kms, BANK, NODE, Some(r#"{"subject_id":"bank-seal"}"#)),
        None
    );

    // A well-formed envelope whose ciphertext was edited.
    let edited = tampered(&envelope);
    assert!(
        CiphertextEnvelope::from_storage_string(&edited).is_ok(),
        "the tampered body still parses, so the tag is what rejects it"
    );
    assert_eq!(open_gist(&kms, BANK, NODE, Some(&edited)), None);

    // A body that was not sealed for this field.
    let other_field = kms
        .encrypt("memtree.other_field", NODE, BANK, GIST.as_bytes())
        .expect("seal under another field")
        .to_storage_string();
    assert_eq!(open_gist(&kms, BANK, NODE, Some(&other_field)), None);
}

#[test]
fn open_gist_rejects_another_bank() {
    let kms = LocalDevKms::new();
    let stored = sealed(&kms, BANK, NODE, GIST);

    assert_eq!(open_gist(&kms, "bank-other", NODE, Some(&stored)), None);
}

#[test]
fn open_gist_rejects_another_node() {
    let kms = LocalDevKms::new();
    let stored = sealed(&kms, BANK, NODE, GIST);

    assert_eq!(open_gist(&kms, BANK, "node-2", Some(&stored)), None);
}

#[test]
fn a_destroyed_dek_leaves_reads_tolerant_and_writes_failing() {
    let kms = LocalDevKms::new();
    let stored = sealed(&kms, BANK, NODE, GIST);
    kms.destroy_dek(BANK).expect("shred");

    assert_eq!(
        open_gist(&kms, BANK, NODE, Some(&stored)),
        None,
        "a crypto-shredded bank reads as no gist rather than an error"
    );
    assert!(
        seal_gist(&kms, &node(BANK, NODE, Some(GIST))).is_err(),
        "the seal path does not resurrect a destroyed DEK"
    );
}
