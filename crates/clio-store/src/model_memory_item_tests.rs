// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Memory-item CRUD, dual representation, episodic/source rules, and legacy-format row coverage.

use crate::async_store::{AsyncStore, BlockingAsyncStore};
use crate::batch_model::BatchStore;
use crate::dek::{DekProvider, LocalDevKms};
use crate::erase_test_support::restore_legacy_format_row;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory, SourceType,
};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

fn sample_semantic(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.0),
        source_ref: Some("turn_9".into()),
        context: Some("team chat".into()),
        snapshot: Some(json!({"port": 8080})),
        gist: Some("service listens on 8080".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
    }
}

fn suite_crud_dual(store: &dyn Store, bank_a: &str, bank_b: &str, id: &str) {
    let item = sample_semantic(id, bank_a);
    store.create_memory_item(&item, "actor-1").unwrap();
    let got = store.get_memory_item(bank_a, id).unwrap().expect("present");
    assert_eq!(got.snapshot, item.snapshot);
    assert_eq!(got.gist, item.gist);
    assert_eq!(got.context.as_deref(), Some("team chat"));
    assert!(store.get_memory_item(bank_b, id).unwrap().is_none());
    let snap = store.get_snapshot(bank_a, id).unwrap().unwrap();
    assert_eq!(snap, json!({"port": 8080}));
    let gist = store.get_gist(bank_a, id).unwrap().unwrap();
    assert_eq!(gist, "service listens on 8080");
    assert_ne!(format!("{snap}"), gist);
    let raw = store
        .inspect_content_ciphertext(bank_a, id)
        .unwrap()
        .unwrap();
    assert!(!raw.contains("8080"));
    assert!(!raw.contains("listens"));
    assert!(!raw.contains("team chat"), "context must be sealed too");
}

fn suite_episodic_fr14(store: &dyn Store, bank_a: &str) {
    let epi_id = unique("epi");
    let mut epi = sample_semantic(&epi_id, bank_a);
    epi.kind = ItemKind::Episodic;
    epi.category = None;
    epi.episodic_type = Some(EpisodicType::Task);
    epi.snapshot = Some(json!({"step": 1}));
    epi.gist = Some("did step 1".into());
    store.create_memory_item(&epi, "actor-1").unwrap();
    assert_eq!(
        store
            .get_memory_item(bank_a, &epi_id)
            .unwrap()
            .unwrap()
            .episodic_type,
        Some(EpisodicType::Task)
    );
    let belief_id = unique("bel");
    let mut belief = sample_semantic(&belief_id, bank_a);
    belief.epistemic_kind = EpistemicKind::Belief;
    assert_eq!(
        store
            .create_memory_item(&belief, "actor-1")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    belief.source_type = Some(SourceType::UserStated);
    store.create_memory_item(&belief, "actor-1").unwrap();
    assert_eq!(
        store
            .get_memory_item(bank_a, &belief_id)
            .unwrap()
            .unwrap()
            .source_type,
        Some(SourceType::UserStated)
    );
    let mut fact_bad = sample_semantic(&unique("fact-bad"), bank_a);
    fact_bad.source_type = Some(SourceType::ThirdParty);
    assert!(store.create_memory_item(&fact_bad, "a").is_err());
}

fn suite_halves_update(store: &dyn Store, bank_a: &str, id: &str) {
    let half_id = unique("half");
    let mut half = sample_semantic(&half_id, bank_a);
    half.snapshot = None;
    half.gist = Some("gist only".into());
    store.create_memory_item(&half, "actor-1").unwrap();
    assert_eq!(
        store.get_snapshot(bank_a, &half_id).unwrap_err().message,
        "snapshot_missing"
    );
    assert_eq!(
        store.get_gist(bank_a, &half_id).unwrap().unwrap(),
        "gist only"
    );
    let half2 = unique("half2");
    let mut half_b = sample_semantic(&half2, bank_a);
    half_b.gist = None;
    store.create_memory_item(&half_b, "actor-1").unwrap();
    assert_eq!(
        store.get_gist(bank_a, &half2).unwrap_err().message,
        "gist_missing"
    );
    assert!(store.get_snapshot(bank_a, "missing").unwrap().is_none());
    assert!(store.get_gist(bank_a, "missing").unwrap().is_none());
    let item = sample_semantic(id, bank_a);
    let mut updated = item.clone();
    updated.gist = Some("revised gist".into());
    updated.updated_at = "t1".into();
    store.update_memory_item(&updated, "actor-2").unwrap();
    assert_eq!(store.get_gist(bank_a, id).unwrap().unwrap(), "revised gist");
    assert!(
        store
            .update_memory_item(&sample_semantic("nope", bank_a), "a")
            .is_err()
    );
    assert!(store.create_memory_item(&item, "actor-1").is_err());
    assert!(store.create_memory_item(&item, "").is_err());
    assert!(store.update_memory_item(&updated, "").is_err());
    // The subject anchors the DEK and erasure scope: updates cannot move an
    // item to a different subject.
    let mut moved = updated.clone();
    moved.subject_id = "other-subject".into();
    let err = store.update_memory_item(&moved, "actor-2").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("cannot change subject_id"), "{err}");
}

/// A legacy-format row fails closed on the typed reader: the body decrypts,
/// so the failure is the row's own format, not the backend.
fn suite_legacy_row<S: Store + BatchStore>(store: &S, kms: &dyn DekProvider, bank: &str) {
    let id = unique("legacy");
    restore_legacy_format_row(store, kms, &id, bank, "subj");
    let err = store.get_memory_item(bank, &id).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnreadableContent);
    assert!(err.message.contains("content_format_mismatch"), "{err}");
}

fn suite_rapid_updates_unique_audit(store: &dyn Store, bank: &str, id: &str) {
    // Back-to-back updates of one item land in the same millisecond; the audit
    // id must stay unique or the `audit_events` primary key insert fails.
    for n in 0..25u32 {
        let mut item = sample_semantic(id, bank);
        item.updated_at = "t-rapid".into();
        item.gist = Some(format!("rapid {n}"));
        store
            .update_memory_item(&item, "actor-rapid")
            .unwrap_or_else(|e| panic!("rapid update {n} failed: {e}"));
    }
}

fn run_memory_suite<S: Store + BatchStore>(store: &S, kms: &dyn DekProvider) {
    let bank_a = unique("mb-a");
    let bank_b = unique("mb-b");
    let id = unique("mem");
    suite_crud_dual(store, &bank_a, &bank_b, &id);
    suite_episodic_fr14(store, &bank_a);
    suite_halves_update(store, &bank_a, &id);
    suite_rapid_updates_unique_audit(store, &bank_a, &id);
    suite_legacy_row(store, kms, &bank_a);
}

fn assert_null_ciphertext_fails(store: &dyn Store, bank: &str, id: &str) {
    // A row with no content is unreadable content, not a backend failure, so a
    // tolerant reader may skip it (see `AmError::is_unreadable_content`).
    assert_eq!(
        store.get_memory_item(bank, id).unwrap_err().code,
        ErrorCode::UnreadableContent
    );
}

#[test]
fn sqlite_memory_item_suite() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    run_memory_suite(&store, kms.as_ref());
    let id = unique("null-ct");
    store
        .create_memory_item(&sample_semantic(&id, "b"), "actor")
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = '' WHERE id = '{id}'"
        ))
        .unwrap();
    assert_null_ciphertext_fails(&store, "b", &id);
}

#[test]
fn postgres_memory_item_suite() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = crate::postgres::PostgresStore::open(&url, Arc::clone(&kms))
        .unwrap_or_else(|e| panic!("postgres required: {e}"));
    run_memory_suite(&store, kms.as_ref());
    let id = unique("null-ct-pg");
    store
        .create_memory_item(&sample_semantic(&id, "b"), "actor")
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = '' WHERE id = '{id}'"
        ))
        .unwrap();
    assert_null_ciphertext_fails(&store, "b", &id);
}

#[tokio::test]
async fn async_memory_item_round_trip() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = Arc::new(SqliteStore::open_in_memory(kms).unwrap());
    let async_store = BlockingAsyncStore::new(store);
    let id = unique("async-mem");
    let item = sample_semantic(&id, "b");
    async_store
        .create_memory_item(item.clone(), "actor".into())
        .await
        .unwrap();
    let got = async_store
        .get_memory_item("b".into(), id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.snapshot, item.snapshot);
    let snap = async_store
        .get_snapshot("b".into(), id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snap["port"], 8080);
    let gist = async_store
        .get_gist("b".into(), id.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(gist.contains("8080"));
    let mut upd = item;
    upd.admission_score = Some(0.5);
    upd.updated_at = "t2".into();
    async_store
        .update_memory_item(upd, "actor".into())
        .await
        .unwrap();
}
