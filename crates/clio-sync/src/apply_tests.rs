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

use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation};
use crate::protocol::{ENTITY_KIND_ASSOC, ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use clio_store::{
    AssocEdgeRecord, AssocKind, DeadLetterStatus, DekProvider, LocalDevKms, SqliteStore, Store,
    SyncStore,
};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) fn fixture() -> (Arc<SqliteStore>, Arc<dyn DekProvider>) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("sqlite open"));
    (store, kms)
}

pub(crate) fn item(bank: &str, id: &str, subject: &str, stamp: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("ev".into()),
        snapshot: Some(json!({ "k": id })),
        gist: Some(gist.into()),
        valid_from: stamp.into(),
        valid_until: None,
        tx_from: stamp.into(),
        tx_until: None,
        created_at: stamp.into(),
        updated_at: stamp.into(),
        context: None,
    }
}

pub(crate) fn item_value(bank: &str, id: &str, subject: &str, stamp: &str, gist: &str) -> Value {
    serde_json::to_value(item(bank, id, subject, stamp, gist)).expect("serialize item")
}

pub(crate) fn hash_of(content: &Value) -> String {
    let text = serde_json::to_string(content).unwrap_or_default();
    clio_store::sha256_hex("am-sync-hash".as_bytes(), &text)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn env(
    mid: &str,
    bank: &str,
    kind: &str,
    entity: &str,
    op: &str,
    stamp: &str,
    dev: &str,
    content: Value,
) -> SyncMutation {
    let content_hash = hash_of(&content);
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: bank.into(),
        entity_kind: kind.into(),
        entity_id: entity.into(),
        op: op.into(),
        transaction_time: stamp.into(),
        device_id: dev.into(),
        content: SyncContent::Plain(content),
        content_hash,
        admission_receipt: None,
    }
}

pub(crate) fn upsert(mid: &str, bank: &str, stamp: &str, dev: &str, gist: &str) -> SyncMutation {
    env(
        mid,
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        stamp,
        dev,
        item_value(bank, "i-1", "s1", stamp, gist),
    )
}

pub(crate) fn edge(
    bank: &str,
    id: &str,
    source: &str,
    target: &str,
    stamp: &str,
) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: bank.into(),
        source_id: source.into(),
        target_id: target.into(),
        assoc_kind: AssocKind::Coactivation,
        relationship: None,
        weight: 0.5,
        last_updated_at: stamp.into(),
    }
}

pub(crate) fn edge_value(bank: &str, id: &str, source: &str, target: &str, stamp: &str) -> Value {
    serde_json::to_value(edge(bank, id, source, target, stamp)).expect("serialize edge")
}

pub(crate) fn assoc(
    mid: &str,
    bank: &str,
    entity: &str,
    op: &str,
    stamp: &str,
    dev: &str,
    content: Value,
) -> SyncMutation {
    env(
        mid,
        bank,
        ENTITY_KIND_ASSOC,
        entity,
        op,
        stamp,
        dev,
        content,
    )
}

pub(crate) fn apply(
    store: &SqliteStore,
    kms: Option<&dyn DekProvider>,
    m: &SyncMutation,
) -> ApplyOutcome {
    apply_result(store, kms, m).expect("apply succeeds")
}

pub(crate) fn apply_result(
    store: &SqliteStore,
    kms: Option<&dyn DekProvider>,
    m: &SyncMutation,
) -> Result<ApplyOutcome, clio_types::AmError> {
    let engine = ApplyEngine {
        store,
        sync: store,
        device_id: "node-1".into(),
        kms,
    };
    let envelope = serde_json::to_string(m).expect("serialize envelope");
    apply_mutation(&engine, m, &envelope)
}

pub(crate) fn receipt(m: &SyncMutation, score: f64) -> SyncMutation {
    SyncMutation {
        admission_receipt: Some(json!({ "admitted": true, "score": score })),
        ..m.clone()
    }
}

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
    assert_eq!(
        apply(&store, None, &m),
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
