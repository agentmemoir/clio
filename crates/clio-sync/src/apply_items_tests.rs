// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the item apply effects owned by
//! `apply_items`: the payload decode, identity, bank, and shredded-subject
//! checks, the preserve-on-absent context rule, the last-writer-wins update and
//! create paths, the malformed payload stamp refusal, and the invalidate and
//! discard writes.
//!
//! ## Owns
//! - Behavior coverage for `apply_item_upsert`, `apply_item_invalidate`, and
//!   `apply_item_discard`
//!
//! ## Does not own
//! - Dispatch, dedupe, and journaling (`apply_tests`)
//! - The record and history effects (`apply_records_tests`,
//!   `apply_history_tests`)
//!
//! ## Boundary
//!
//! Tests here break when an item apply effect changes; they never change it.
//!
//! Test-only. Never compiled into production builds.

use crate::apply::ApplyOutcome;
use crate::apply_fixtures::{apply, env, fixture, item, item_value, upsert};
use crate::crypto::SyncCipherKey;
use crate::protocol::{
    ENTITY_KIND_ITEM, OP_DISCARD, OP_INVALIDATE, OP_UPSERT, REASON_MALFORMED,
    REASON_MALFORMED_STAMP, SyncContent,
};
use clio_store::DeadLetterStatus;
use clio_store::Store as _;
use clio_store::SyncStore as _;
use serde_json::json;

/// The store keeps the payload's `updated_at` verbatim as the row's stamp, so a
/// peer-supplied value this build cannot place must be refused before any
/// write: otherwise it becomes the stored side of every later comparison.
#[test]
fn a_peer_payload_updated_at_must_be_placeable_before_it_is_stored() {
    let (store, _kms) = fixture();
    let bank = "b-payload";
    let canonical = upsert(
        "m-canonical",
        bank,
        "2026-01-01T00:00:00Z",
        "dev-a",
        "canonical",
    );
    assert_eq!(apply(&store, None, &canonical), ApplyOutcome::Applied);
    let mut value = item_value(bank, "i-1", "s1", "2026-01-02T00:00:00Z", "poison");
    value["updated_at"] = json!("zzzzzzzzzzzzzzzzzzzz");
    let poison = env(
        "m-poison",
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        "2026-01-02T00:00:00Z",
        "dev-z",
        value,
    );
    assert_eq!(
        apply(&store, None, &poison),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_MALFORMED_STAMP.into()
        }
    );
    let row = store
        .get_memory_item(bank, "i-1")
        .unwrap()
        .expect("the canonical row stays");
    assert_eq!(row.gist.as_deref(), Some("canonical"));
    assert_eq!(row.updated_at, "2026-01-01T00:00:00Z");
}

/// The envelope entity selects the row the LWW comparison reads, so it must
/// name the same row the payload writes; otherwise one row is compared and
/// another is overwritten with no ordering decision.
#[test]
fn an_item_payload_id_must_match_the_envelope_entity() {
    let (store, _kms) = fixture();
    let bank = "b-identity";
    let a = env(
        "m-a",
        bank,
        ENTITY_KIND_ITEM,
        "i-a",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        item_value(bank, "i-a", "s1", "2026-01-01T00:00:00Z", "a"),
    );
    let b = env(
        "m-b",
        bank,
        ENTITY_KIND_ITEM,
        "i-b",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        item_value(bank, "i-b", "s1", "2026-01-01T00:00:00Z", "b"),
    );
    assert_eq!(apply(&store, None, &a), ApplyOutcome::Applied);
    assert_eq!(apply(&store, None, &b), ApplyOutcome::Applied);
    let split = env(
        "m-split",
        bank,
        ENTITY_KIND_ITEM,
        "i-a",
        OP_UPSERT,
        "2026-01-02T00:00:00Z",
        "dev-z",
        item_value(bank, "i-b", "s1", "2026-01-02T00:00:00Z", "poison"),
    );
    assert_eq!(
        apply(&store, None, &split),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_MALFORMED.into()
        }
    );
    let stored_a = store
        .get_memory_item(bank, "i-a")
        .unwrap()
        .expect("row A stays");
    assert_eq!(stored_a.gist.as_deref(), Some("a"));
    let stored_b = store
        .get_memory_item(bank, "i-b")
        .unwrap()
        .expect("row B stays");
    assert_eq!(stored_b.gist.as_deref(), Some("b"));
}

#[test]
fn t24_09_erased_subject_resurrection_dead_letters() {
    let (store, _kms) = fixture();
    store
        .upsert_subject_key_ref("subj-gone", "shredded", "shredded")
        .expect("shred marker");
    let m = env(
        "m-res",
        "b-erase",
        ENTITY_KIND_ITEM,
        "i-res",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        item_value(
            "b-erase",
            "i-res",
            "subj-gone",
            "2026-01-01T00:00:00Z",
            "resurrected",
        ),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "erased_subject".into()
        }
    );
    let open = store
        .dead_letter_list("b-erase", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].reason_code, "erased_subject");
    assert!(store.get_memory_item("b-erase", "i-res").unwrap().is_none());
}

#[test]
fn t24_08_item_bank_mismatch_dead_letters_unauthorized_bank() {
    let (store, _kms) = fixture();
    let m = env(
        "m-x",
        "bank-wire",
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        item_value("bank-item", "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: "unauthorized_bank".into()
        }
    );
}

#[test]
fn item_upsert_carries_context_and_pre_field_payloads_still_apply() {
    let (store, _kms) = fixture();
    let mut payload = item_value("b-ctx-sync", "i-1", "s1", "2026-01-01T00:00:01Z", "g1");
    payload["context"] = json!("team chat");
    let m = env(
        "m-ctx",
        "b-ctx-sync",
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        "2026-01-01T00:00:01Z",
        "dev-a",
        payload,
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    let got = store
        .get_memory_item("b-ctx-sync", "i-1")
        .unwrap()
        .expect("applied item");
    assert_eq!(got.context.as_deref(), Some("team chat"));

    // A payload produced before the field existed (no `context` key at all)
    // applies unchanged and stores an absent context.
    let legacy = item_value("b-ctx-legacy", "i-2", "s1", "2026-01-01T00:00:02Z", "g2");
    assert!(legacy.get("context").is_none());
    let m2 = env(
        "m-legacy",
        "b-ctx-legacy",
        ENTITY_KIND_ITEM,
        "i-2",
        OP_UPSERT,
        "2026-01-01T00:00:02Z",
        "dev-a",
        legacy,
    );
    assert_eq!(apply(&store, None, &m2), ApplyOutcome::Applied);
    let got2 = store
        .get_memory_item("b-ctx-legacy", "i-2")
        .unwrap()
        .expect("legacy applied item");
    assert!(got2.context.is_none());
}

#[test]
fn invalidate_missing_noop_existing_closes_and_discard_dedupes() {
    let (store, _kms) = fixture();
    let bank = "b-life";
    let miss = env(
        "m-miss",
        bank,
        ENTITY_KIND_ITEM,
        "nope",
        OP_INVALIDATE,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({}),
    );
    assert_eq!(apply(&store, None, &miss), ApplyOutcome::Applied);
    store
        .create_memory_item(
            &item(bank, "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
            "actor",
        )
        .unwrap();
    let close = env(
        "m-close",
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_INVALIDATE,
        "2026-01-02T00:00:00Z",
        "dev-a",
        json!({}),
    );
    assert_eq!(apply(&store, None, &close), ApplyOutcome::Applied);
    let got = store.get_memory_item(bank, "i-1").unwrap().expect("item");
    assert!(got.tx_until.is_some());
    assert!(got.valid_until.is_some());
    let first = env(
        "m-d1",
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_DISCARD,
        "2026-01-03T00:00:00Z",
        "dev-a",
        json!({ "r": 1 }),
    );
    assert_eq!(apply(&store, None, &first), ApplyOutcome::Applied);
    // A repeat discard reads as "missing" (identity lookup excludes discarded
    // rows), so the engine takes the Applied no-op path, not Duplicate.
    let mut again = first;
    again.sync_mutation_id = "m-d2".into();
    again.content_hash = "other-hash".into();
    assert_eq!(apply(&store, None, &again), ApplyOutcome::Applied);
    assert!(store.get_item_identity(bank, "i-1").unwrap().is_none());
}

#[test]
fn a_sealed_item_payload_applies_without_a_plain_write() {
    let (store, kms) = fixture();
    let key = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("items-cipher"));
    let mut m = upsert("m-cipher", "b-cipher", "2026-01-01T00:00:00Z", "dev-a", "g");
    m.content = SyncContent::Cipher(key.seal(br"{}", &m.execution_view().aad()).expect("seal"));
    assert_eq!(apply(&store, Some(kms.as_ref()), &m), ApplyOutcome::Applied);
    assert!(store.get_memory_item("b-cipher", "i-1").unwrap().is_none());
}

#[test]
fn an_undecodable_item_payload_is_dead_lettered_as_malformed() {
    let (store, _kms) = fixture();
    let m = env(
        "m-bad-json",
        "b-bad-json",
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        json!({}),
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_MALFORMED.into()
        }
    );
}

#[test]
fn an_item_payload_without_a_time_axis_is_dead_lettered() {
    let (store, _kms) = fixture();
    let mut value = item_value("b-invalid", "i-1", "s1", "2026-01-01T00:00:00Z", "g");
    value["valid_from"] = json!("");
    let m = env(
        "m-invalid",
        "b-invalid",
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        "2026-01-01T00:00:00Z",
        "dev-a",
        value,
    );
    assert_eq!(
        apply(&store, None, &m),
        ApplyOutcome::DeadLettered {
            reason_code: REASON_MALFORMED.into()
        }
    );
}

#[test]
fn an_older_item_payload_loses_last_writer_wins() {
    let (store, _kms) = fixture();
    let bank = "b-items-lww";
    let newer = upsert("m-newer", bank, "2026-01-02T00:00:00Z", "dev-b", "newer");
    let older = upsert("m-older", bank, "2026-01-01T00:00:00Z", "dev-a", "older");
    assert_eq!(apply(&store, None, &newer), ApplyOutcome::Applied);
    assert_eq!(apply(&store, None, &older), ApplyOutcome::ConflictLoser);
    let got = store.get_memory_item(bank, "i-1").unwrap().expect("item");
    assert_eq!(got.gist.as_deref(), Some("newer"));
}
