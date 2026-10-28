// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite index SQL: the outbox lifecycle, the keyed
//! lexical leg, the widened dense leg, coverage reads, and the orphan sweep.
//!
//! ## Owns
//! - Outbox enqueue/finish/fail assertions with attempts and backoff filtering
//! - Lexical and dense search, bank isolation, domain filters, and fail-closed
//!   validation
//! - Coverage, indexable windows, and the bank-scoped orphan sweep
//!
//! ## Does not own
//! - Production behaviour
//! - The vector-table orphan-sweep SQL (`vector_backend`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the SQLite index SQL.

use crate::dek::{DekProvider, LocalDevKms};
use crate::index_store::IndexStore;
use crate::model::{EMBEDDING_DIMS, VectorSlot};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{
    EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SearchDomain, SemanticCategory,
};
use serde_json::json;
use std::sync::Arc;

fn item(id: &str, bank: &str, kind: ItemKind, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subject-{bank}"),
        kind,
        category: if kind == ItemKind::Semantic {
            Some(SemanticCategory::Schema)
        } else {
            None
        },
        episodic_type: if kind == ItemKind::Episodic {
            Some(EpisodicType::Task)
        } else {
            None
        },
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.6),
        source_ref: Some("index-test".into()),
        context: None,
        snapshot: Some(json!({"gist": gist})),
        gist: Some(gist.into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn unit_vector(axis: usize) -> Vec<f32> {
    let mut embedding = vec![0.0_f32; EMBEDDING_DIMS];
    embedding[axis] = 1.0;
    embedding
}

fn vector(store: &SqliteStore, id: &str, bank: &str, axis: usize) {
    store
        .upsert_vector(&VectorSlot {
            item_id: id.into(),
            bank_id: bank.into(),
            model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
            embedding: unit_vector(axis),
        })
        .expect("upsert vector");
}

#[test]
fn the_outbox_lifecycle_enqueues_once_and_finishes_clean() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-outbox");
    let id = unique("item");
    store
        .create_memory_item(&item(&id, &bank, ItemKind::Semantic, "leaf"), "agent")
        .expect("create");

    assert!(
        store
            .pending_index_job(&bank, &id, None, u32::MAX)
            .expect("one pending job")
            .is_some(),
        "create must enqueue the index job"
    );
    assert!(
        !store
            .enqueue_index_job(&bank, &id)
            .expect("duplicate enqueue"),
        "a duplicate enqueue before finish must be a no-op"
    );

    store.finish_index_job(&bank, &id).expect("finish");
    assert!(
        store
            .pending_index_job(&bank, &id, None, u32::MAX)
            .expect("finished job")
            .is_none(),
        "finish must remove the job row"
    );
    assert!(
        store.enqueue_index_job(&bank, &id).expect("re-enqueue"),
        "a fresh enqueue after finish must insert again"
    );

    let err = store
        .enqueue_index_job("", &id)
        .expect_err("an empty bank must fail closed");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
}

#[test]
fn a_failed_job_records_attempts_and_respects_the_max_attempts_cap() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-fail");
    let id = unique("item");
    store.enqueue_index_job(&bank, &id).expect("enqueue");

    store
        .fail_index_job(&bank, &id, "embedder offline")
        .expect("fail");
    let job = store
        .pending_index_job(&bank, &id, None, 2)
        .expect("job below the cap")
        .expect("one attempt must stay below a cap of 2");
    assert_eq!(job.attempts, 1, "{job:?}");
    assert_eq!(
        job.last_error.as_deref(),
        Some("embedder offline"),
        "{job:?}"
    );
    assert!(
        store
            .pending_index_job(&bank, &id, None, 1)
            .expect("capped read")
            .is_none(),
        "a job at the attempts cap must not be served again"
    );
}

#[test]
fn the_lexical_leg_is_bank_scoped_and_fail_closed() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-lexical");
    let other = unique("bank-lexical-other");
    let semantic = unique("fact");
    let task = unique("task");
    store
        .create_memory_item(
            &item(&semantic, &bank, ItemKind::Semantic, "a fact"),
            "agent",
        )
        .expect("create fact");
    store
        .create_memory_item(&item(&task, &bank, ItemKind::Episodic, "a task"), "agent")
        .expect("create task");

    let err = store
        .put_lexical_doc(&bank, &unique("absent"), "no item")
        .expect_err("a doc for a missing item must fail");
    assert_eq!(err.code, ErrorCode::ItemNotFound, "{err}");

    store
        .put_lexical_doc(&bank, &semantic, "schema fact stored")
        .expect("lexical doc for the fact");
    store
        .put_lexical_doc(&bank, &task, "schema task stored")
        .expect("lexical doc for the task");

    let hits = store
        .lexical_search(&bank, "stored", &[], 5)
        .expect("lexical hits");
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(
        store
            .lexical_search(&other, "stored", &[], 5)
            .expect("cross-bank read")
            .is_empty(),
        "cross-bank lexical search must fail closed"
    );
    let tasks = store
        .lexical_search(&bank, "stored", &[SearchDomain::Task], 5)
        .expect("domain-filtered hits");
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].item_id, task, "{tasks:?}");
    let err = store
        .lexical_search(&bank, "   ", &[], 5)
        .expect_err("an empty query must fail closed");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    assert!(
        store
            .lexical_search(&bank, "stored", &[], 0)
            .expect("zero limit")
            .is_empty(),
        "limit 0 must return no lexical hits"
    );
    assert!(
        store.lexical_search(&other, "stored", &[], 5).is_ok(),
        "an empty index must answer without key material"
    );

    assert!(
        store
            .delete_lexical_doc(&bank, &task)
            .expect("delete the task doc"),
        "delete must report the removed doc"
    );
    assert!(
        !store
            .delete_lexical_doc(&bank, &task)
            .expect("repeat delete"),
        "a repeated lexical delete must report false"
    );
    let hits = store
        .lexical_search(&bank, "stored", &[], 5)
        .expect("hits after delete");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].item_id, semantic, "{hits:?}");
}

#[test]
fn the_dense_leg_widens_filters_domains_and_fails_closed() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-dense");
    let other = unique("bank-dense-other");
    let semantic = unique("fact");
    let task = unique("task");
    store
        .create_memory_item(
            &item(&semantic, &bank, ItemKind::Semantic, "a fact"),
            "agent",
        )
        .expect("create fact");
    store
        .create_memory_item(&item(&task, &bank, ItemKind::Episodic, "a task"), "agent")
        .expect("create task");
    vector(&store, &semantic, &bank, 0);
    vector(&store, &task, &bank, 1);

    let hits = store
        .dense_search(&bank, &unit_vector(0), &[], 2)
        .expect("dense");
    let ids: Vec<&str> = hits.iter().map(|h| h.item_id.as_str()).collect();
    assert_eq!(ids, vec![semantic.as_str(), task.as_str()], "{hits:?}");
    assert!(
        store
            .dense_search(&other, &unit_vector(0), &[], 2)
            .expect("cross-bank dense")
            .is_empty(),
        "cross-bank dense search must fail closed"
    );
    let tasks = store
        .dense_search(&bank, &unit_vector(0), &[SearchDomain::Task], 2)
        .expect("domain-filtered dense");
    let task_ids: Vec<&str> = tasks.iter().map(|h| h.item_id.as_str()).collect();
    assert_eq!(task_ids, vec![task.as_str()], "{tasks:?}");
    assert!(
        store
            .dense_search(&bank, &unit_vector(0), &[SearchDomain::Persona], 2)
            .expect("persona-filtered dense")
            .is_empty(),
        "a domain no item belongs to must return no dense hits"
    );
    assert!(
        store
            .dense_search(&bank, &unit_vector(0)[..10], &[], 1)
            .is_err(),
        "a wrong-dimension query must fail closed"
    );
    assert!(
        store
            .dense_search(&bank, &unit_vector(0), &[], 0)
            .expect("zero limit")
            .is_empty(),
        "limit 0 must return no dense hits"
    );
    let third = unique("third");
    store
        .create_memory_item(&item(&third, &bank, ItemKind::Semantic, "three"), "agent")
        .expect("create third");
    vector(&store, &third, &bank, 0);
    let widened = store
        .dense_search(&bank, &unit_vector(0), &[], 2)
        .expect("widened dense");
    let ids: Vec<&str> = widened.iter().map(|h| h.item_id.as_str()).collect();
    assert_eq!(widened.len(), 2, "{widened:?}");
    assert!(ids.contains(&semantic.as_str()), "{widened:?}");
    assert!(ids.contains(&third.as_str()), "{widened:?}");
    assert!(!ids.contains(&task.as_str()), "{widened:?}");
}

#[test]
fn coverage_reads_and_indexable_windows_report_the_bank() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-coverage");
    let first = unique("first");
    let second = unique("second");
    store
        .create_memory_item(&item(&first, &bank, ItemKind::Semantic, "one"), "agent")
        .expect("create one");
    store
        .create_memory_item(&item(&second, &bank, ItemKind::Episodic, "two"), "agent")
        .expect("create two");

    let page = store.indexable_items(&bank, 1, 1).expect("indexable page");
    assert_eq!(page, vec![second.clone()], "{page:?}");
    assert_eq!(
        store.count_indexable_items(&bank).expect("count"),
        2,
        "both open items are indexable"
    );
    assert!(
        store
            .indexable_banks()
            .expect("indexable banks")
            .iter()
            .any(|b| b == &bank),
        "the bank must be listed as indexable"
    );
    let coverage = store.index_coverage(Some(&bank)).expect("coverage");
    assert_eq!(coverage.pending_jobs, 2, "{coverage:?}");
}

#[test]
fn the_orphan_sweep_removes_only_rows_whose_item_is_gone() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank_a = unique("bank-sweep-a");
    let bank_b = unique("bank-sweep-b");
    let orphan = unique("orphan");
    let kept = unique("kept");
    store
        .create_memory_item(&item(&orphan, &bank_a, ItemKind::Semantic, "gone"), "agent")
        .expect("create orphan");
    store
        .create_memory_item(&item(&kept, &bank_b, ItemKind::Semantic, "kept"), "agent")
        .expect("create kept");
    vector(&store, &orphan, &bank_a, 0);
    vector(&store, &kept, &bank_b, 0);
    store
        .put_lexical_doc(&bank_a, &orphan, "orphan text")
        .expect("lexical doc");
    // Leave the derived rows behind by removing the item row directly: the
    // sweep exists for writers that never ran the cascade.
    store
        .test_execute(&format!(
            "DELETE FROM items WHERE id = '{orphan}' AND bank_id = '{bank_a}'"
        ))
        .expect("drop the orphan item row");

    assert_eq!(
        store.cleanup_orphans(Some(&bank_a)).expect("scoped sweep"),
        3,
        "the vector, lexical, and outbox rows of the orphan must go"
    );
    assert!(
        store
            .pending_index_job(&bank_a, &orphan, None, u32::MAX)
            .expect("orphan outbox read")
            .is_none(),
        "the orphan's outbox row must be gone"
    );
    assert!(
        store
            .dense_search(&bank_b, &unit_vector(0), &[], 5)
            .expect("kept bank after scoped sweep")
            .iter()
            .all(|hit| hit.item_id == kept),
        "a scoped sweep must spare other banks"
    );
    assert_eq!(
        store.cleanup_orphans(None).expect("unscoped sweep"),
        0,
        "no orphans may remain after the scoped sweep"
    );
}
