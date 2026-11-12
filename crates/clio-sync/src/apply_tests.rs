// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync apply engine at the engine level (SQLite
//! in-memory, no transport): idempotent dedupe on `sync_mutation_id`, the
//! `sync_lww_rule` winner/loser matrix, `remote_admission_trust` (no local
//! re-scoring), the dead-letter quarantine with reason codes, and every
//! entity-kind / op branch of the apply matrix.
//!
//! ## Owns
//! - Behavior coverage for `apply`: dedupe, LWW conflicts, DLQ, op matrix
//!
//! ## Does not own
//! - Transport integration (`server_tests` / `client_tests`)
//! - Wire contract unit tests (`protocol_tests`)
//!
//! ## Boundary
//!
//! Tests here break when apply semantics change; they never change it.
//!
//! Keep this module focused on apply engine tests.

use crate::apply::ApplyOutcome;
use crate::apply_fixtures::{apply, apply_at_epoch, env, fixture, item_value, receipt, upsert};
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT};
use clio_store::{DeadLetterStatus, SqliteStore, Store, SyncStore};
use serde_json::json;

#[test]
fn t24_02_same_mutation_id_applies_once_then_duplicates() {
    let (store, _kms) = fixture();
    let m = upsert("m-1", "b-idem", "2026-01-01T00:00:01Z", "dev-a", "g1");
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Duplicate);
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Duplicate);
    assert!(store.get_memory_item("b-idem", "i-1").unwrap().is_some());
    assert!(store.journal_get("b-idem", "m-1").unwrap().is_some());
    assert_eq!(store.journal_max_seq("b-idem").unwrap(), 1);
    // Secondary guard: a different id but the same (kind, entity, op, hash).
    let mut replay = m;
    replay.sync_mutation_id = "m-b".into();
    assert_eq!(apply(&store, None, &replay), ApplyOutcome::Duplicate);
}

#[test]
fn t24_03_lww_later_stamp_wins_loser_conflict_recorded() {
    let (store, kms) = fixture();
    let bank = "b-lww";
    let older = upsert("m-old", bank, "2026-01-01T00:00:00Z", "dev-a", "older");
    let newer = upsert("m-new", bank, "2026-01-02T00:00:00Z", "dev-b", "newer");
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &older),
        ApplyOutcome::Applied
    );
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &newer),
        ApplyOutcome::Applied
    );
    let gist = |store: &SqliteStore| -> Option<String> {
        store
            .get_memory_item(bank, "i-1")
            .unwrap()
            .expect("item")
            .gist
    };
    assert_eq!(gist(&store).as_deref(), Some("newer"));
    let stale = upsert("m-stale", bank, "2026-01-01T12:00:00Z", "dev-late", "stale");
    assert_eq!(
        apply(&store, Some(kms.as_ref()), &stale),
        ApplyOutcome::ConflictLoser
    );
    assert_eq!(gist(&store).as_deref(), Some("newer"));
    assert_eq!(store.conflict_count(bank).unwrap(), 1);
    let conflicts = store.conflict_list(bank, 5).unwrap();
    assert_eq!(conflicts[0].loser_mutation_id, "m-stale");
    assert_eq!(conflicts[0].loser_device_id, "dev-late");
    // The winner row is journaled sealed under the local DEK.
    let row = store
        .journal_get(bank, "m-new")
        .unwrap()
        .expect("journaled");
    assert!(row.content_sealed.is_some());
    assert!(!row.client_encrypted);
}

#[test]
fn t24_03_equal_stamp_tie_broken_by_score_then_device() {
    let (store, _kms) = fixture();
    let bank = "b-tie";
    assert_eq!(
        apply(
            &store,
            None,
            &upsert("m-base", bank, "2026-01-01T00:00:00Z", "dev-a", "base")
        ),
        ApplyOutcome::Applied
    );
    // Same stamp, lower admission score: loses.
    let weak = receipt(
        &upsert("m-weak", bank, "2026-01-01T00:00:00Z", "dev-w", "weak"),
        0.4,
    );
    assert_eq!(apply(&store, None, &weak), ApplyOutcome::ConflictLoser);
    // Same stamp, higher score: wins.
    let strong = receipt(
        &upsert("m-strong", bank, "2026-01-01T00:00:00Z", "dev-s", "strong"),
        0.9,
    );
    assert_eq!(apply(&store, None, &strong), ApplyOutcome::Applied);
    let gist = store
        .get_memory_item(bank, "i-1")
        .unwrap()
        .expect("item")
        .gist;
    assert_eq!(gist.as_deref(), Some("strong"));
    // Score tie at equal stamps: lexicographic device id breaks it. The local
    // row is compared under this node's device id (`node-1`), so a higher
    // device id (`peer-z` > `node-1`) wins the tie.
    let tie = receipt(
        &upsert("m-tie", bank, "2026-01-01T00:00:00Z", "peer-z", "tied"),
        0.5,
    );
    assert_eq!(apply(&store, None, &tie), ApplyOutcome::Applied);
    let gist = store
        .get_memory_item(bank, "i-1")
        .unwrap()
        .expect("item")
        .gist;
    assert_eq!(gist.as_deref(), Some("tied"));
}

#[test]
fn p_no_readmit_divergence_low_score_peer_create_applies() {
    let (store, _kms) = fixture();
    let m = receipt(
        &upsert(
            "m-peer",
            "b-readmit",
            "2026-01-01T00:00:00Z",
            "peer-1",
            "peer gist",
        ),
        0.05,
    );
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    let got = store
        .get_memory_item("b-readmit", "i-1")
        .unwrap()
        .expect("readable");
    assert_eq!(got.gist.as_deref(), Some("peer gist"));
}

#[test]
fn t24_09_malformed_envelope_dead_letters_then_ack_skip_once() {
    let (store, _kms) = fixture();
    let mut m = upsert("m-bad", "b-dlq", "2026-01-01T00:00:00Z", "dev-a", "g");
    m.transaction_time = String::new();
    // The stamp names no instant, so the clock is pinned explicitly: the
    // fixture-defect guard must stay loud for every ordinary case.
    assert_eq!(
        apply_at_epoch(&store, None, &m, 0),
        ApplyOutcome::DeadLettered {
            reason_code: "malformed".into()
        }
    );
    let open = store
        .dead_letter_list("b-dlq", DeadLetterStatus::Open, 10)
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].reason_code, "malformed");
    assert!(
        store
            .dead_letter_ack_skip("m-bad", "2026-01-01T00:00:00Z")
            .unwrap()
    );
    let dlq = store
        .dead_letter_list("b-dlq", DeadLetterStatus::Open, 10)
        .unwrap();
    assert!(dlq.is_empty(), "{dlq:?}");
    assert!(
        !store
            .dead_letter_ack_skip("m-bad", "2026-01-01T00:00:00Z")
            .unwrap()
    );
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

/// The content text the writer hashed, serialized the same way.
fn content_text(m: &crate::protocol::SyncMutation) -> String {
    serde_json::to_string(m.content_value()).expect("serialize content")
}

/// The v2 content digest the current write path produces.
fn v2_content_hash(m: &crate::protocol::SyncMutation) -> String {
    crate::keyed_roles::content_digest(None, &content_text(m))
}

/// The v1 mutation id an older build derived for the same event.
fn v1_mutation_id(m: &crate::protocol::SyncMutation) -> String {
    let preimage = format!("{}|{}|{}", m.entity_id, m.transaction_time, m.op);
    crate::legacy_tags::legacy_sha256_hex(crate::legacy_tags::PLAINTEXT_MUTATION_ID, &preimage)
        .expect("legacy tag data")
}

/// The v2 mutation id the current write path produces.
fn v2_mutation_id(m: &crate::protocol::SyncMutation) -> String {
    crate::keyed_roles::mutation_id(None, &m.entity_id, &m.transaction_time, &m.op)
}

/// A journal row written by an older build holds a v1 digest. The apply guard
/// must still match when a newer build sends the same content under v2.
#[test]
fn stored_v1_content_digest_still_satisfies_the_apply_guard() {
    let (store, _kms) = fixture();
    let bank = "b-guard-legacy";
    let stamp = "2026-01-01T00:00:01Z";
    let mut original = upsert("m-v1-hash", bank, stamp, "dev-a", "g1");
    original.content_hash = crate::legacy_tags::legacy_sha256_hex(
        crate::legacy_tags::PLAINTEXT_DIGEST,
        &content_text(&original),
    )
    .expect("legacy tag data");
    assert_eq!(apply(&store, None, &original), ApplyOutcome::Applied);

    let mut replay = original;
    replay.sync_mutation_id = "m-v2-hash".into();
    replay.content_hash = v2_content_hash(&replay);

    assert_eq!(apply(&store, None, &replay), ApplyOutcome::Duplicate);
    assert_eq!(store.journal_max_seq(bank).expect("max seq"), 1);
    assert!(
        store
            .journal_get(bank, "m-v1-hash")
            .expect("journal read")
            .is_some(),
        "the journal keeps the v1 row",
    );
    assert!(
        store
            .journal_get(bank, "m-v2-hash")
            .expect("journal read")
            .is_none(),
        "the duplicate writes no second row",
    );
}

/// A redelivery can carry the v2 id while the journal holds the v1 id written
/// by an older build. The redelivery is still seen and applies no second time.
#[test]
fn redelivered_v1_mutation_id_is_seen_and_does_not_double_apply() {
    let (store, _kms) = fixture();
    let bank = "b-redeliver-legacy";
    let stamp = "2026-01-01T00:00:01Z";
    let mut older = upsert("placeholder", bank, stamp, "dev-a", "g1");
    older.sync_mutation_id = v1_mutation_id(&older);
    assert_eq!(apply(&store, None, &older), ApplyOutcome::Applied);

    let mut redelivery = older.clone();
    redelivery.sync_mutation_id = v2_mutation_id(&redelivery);
    assert_ne!(redelivery.sync_mutation_id, older.sync_mutation_id);

    assert_eq!(apply(&store, None, &redelivery), ApplyOutcome::Duplicate);
    assert_eq!(store.journal_max_seq(bank).expect("max seq"), 1);
}

/// The v2 values the write path produces still pass both checks.
#[test]
fn current_v2_digest_and_mutation_id_pass_every_guard() {
    let (store, _kms) = fixture();
    let bank = "b-guard-v2";
    let stamp = "2026-01-01T00:00:01Z";
    let mut m = upsert("placeholder", bank, stamp, "dev-a", "g1");
    m.sync_mutation_id = v2_mutation_id(&m);
    m.content_hash = v2_content_hash(&m);

    assert_eq!(apply(&store, None, &m), ApplyOutcome::Applied);
    assert_eq!(apply(&store, None, &m), ApplyOutcome::Duplicate);
    assert_eq!(store.journal_max_seq(bank).expect("max seq"), 1);
}
