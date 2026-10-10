// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared behavioral parity suite of the
//! index pipeline contract across both storage backends.
//!
//! ## Owns
//! - Outbox, lexical, dense, coverage, validation, and orphan-cleanup assertions
//!   run identically against SQLite and Postgres
//!
//! ## Does not own
//! - Backend-specific SQL (`sqlite_index`/`postgres_index`)
//! - Worker scheduling or rebuild policy (owned by the `clio-index` crate)
//!
//! ## Boundary
//!
//! Changes to the `IndexStore` contract should normally update the suite here.
//! Changes to one backend's internals should not require changes here.
//!
//! Keep this module focused on backend-agnostic index contract parity.

use crate::index_store::IndexStore;
use crate::model::{EMBEDDING_DIMS, VectorSlot};
use crate::store::Store;
use clio_types::{
    EpisodicType, EpistemicKind, ItemKind, MemoryItem, SearchDomain, SemanticCategory,
};
use serde_json::json;

/// Combined trait object so one arg exercises both backend contracts.
pub trait IndexBackend: Store + IndexStore {}
impl<T: Store + IndexStore> IndexBackend for T {}

use crate::test_unique::unique;

fn item(id: &str, bank: &str, kind: ItemKind, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{bank}"),
        kind,
        category: if kind == ItemKind::Semantic {
            Some(SemanticCategory::ToolConfig)
        } else {
            None
        },
        episodic_type: match kind {
            ItemKind::Episodic => Some(EpisodicType::Task),
            ItemKind::Semantic => None,
        },
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_1".into()),
        snapshot: Some(json!({"error_code": "ECONNRESET", "service": "deploy-bot"})),
        gist: Some(gist.into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn slot(id: &str, bank: &str, emb: &[f32]) -> VectorSlot {
    let mut v = vec![0.0_f32; EMBEDDING_DIMS];
    for (i, x) in emb.iter().enumerate() {
        v[i] = *x;
    }
    VectorSlot {
        item_id: id.into(),
        bank_id: bank.into(),
        model_id: "BAAI/bge-small-en-v1.5".into(),
        embedding: v,
    }
}

/// Run index-contract assertions against one backend (store and index are the same object).
#[allow(clippy::too_many_lines)]
pub fn run_index_suite(store: &dyn IndexBackend) {
    // These queries are shared-store global (other banks' rows can be queued),
    // so read a window large enough that this test's own rows cannot be squeezed
    // out by unrelated rows from parallel tests.
    const ALL_PENDING: usize = 1_000_000;
    let bank_a = unique("bank-a");
    let bank_b = unique("bank-b");

    // Shape: leaf create durably enqueues the index job (transactional outbox).
    let id_1 = unique("item-1");
    store
        .create_memory_item(
            &item(
                &id_1,
                &bank_a,
                ItemKind::Semantic,
                "deploy failed with ECONNRESET",
            ),
            "actor",
        )
        .unwrap();
    assert!(
        store
            .pending_index_jobs(ALL_PENDING, None, u32::MAX)
            .unwrap()
            .iter()
            .any(|j| j.bank_id == bank_a && j.item_id == id_1),
        "create must enqueue an index job"
    );
    assert!(
        !store.enqueue_index_job(&bank_a, &id_1).unwrap(),
        "duplicate enqueue before finish must be a no-op"
    );

    // Lexical exact token + bank isolation + domain filters.
    let id_2 = unique("task-1");
    store
        .create_memory_item(
            &item(&id_2, &bank_a, ItemKind::Episodic, "retry the task now"),
            "actor",
        )
        .unwrap();
    store
        .put_lexical_doc(&bank_a, &id_1, "deploy failed with ECONNRESET retry")
        .unwrap();
    store
        .put_lexical_doc(&bank_a, &id_2, "retry the task now")
        .unwrap();

    let hits = store.lexical_search(&bank_a, "ECONNRESET", &[], 5).unwrap();
    assert_eq!(hits.first().expect("exact token hit").item_id, id_1);
    assert!(
        store
            .lexical_search(&bank_b, "ECONNRESET", &[], 5)
            .unwrap()
            .is_empty(),
        "cross-bank lexical search must fail closed"
    );

    let all = store.lexical_search(&bank_a, "retry", &[], 5).unwrap();
    assert_eq!(all.len(), 2, "both open items contain `retry`");
    let tasks = store
        .lexical_search(&bank_a, "retry", &[SearchDomain::Task], 5)
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].item_id, id_2);

    // Invalidated items are marked unsearchable (current-belief filter).
    store
        .invalidate_item(&bank_a, &id_2, "t1", "t1", None, "agent")
        .unwrap();
    assert!(
        store
            .lexical_search(&bank_a, "retry", &[], 5)
            .unwrap()
            .iter()
            .all(|h| h.item_id != id_2),
        "invalidated item must drop out of lexical search"
    );

    // Dense search + bank isolation + backend KNN path.
    let mut near = vec![0.0_f32; EMBEDDING_DIMS];
    near[0] = 1.0;
    let mut far = vec![0.0_f32; EMBEDDING_DIMS];
    far[1] = 1.0;
    store.upsert_vector(&slot(&id_1, &bank_a, &near)).unwrap();
    store.upsert_vector(&slot(&id_2, &bank_a, &far)).unwrap();
    let dense = store.dense_search(&bank_a, &near, &[], 2).unwrap();
    assert_eq!(dense.first().expect("dense hit").item_id, id_1);
    assert!(
        store
            .dense_search(&bank_b, &near, &[], 2)
            .unwrap()
            .is_empty(),
        "cross-bank dense search must fail closed"
    );
    let dense_task = store
        .dense_search(&bank_a, &near, &[SearchDomain::Task], 2)
        .unwrap();
    assert!(
        dense_task.iter().all(|h| h.item_id != id_2),
        "invalidated item must drop out of dense search"
    );
    assert!(
        store.dense_search(&bank_a, &near[..10], &[], 1).is_err(),
        "wrong-dimension queries fail closed"
    );

    // Zero limit yields no rows on either leg (never one row from a widened fetch).
    assert!(
        store
            .lexical_search(&bank_a, "retry", &[], 0)
            .unwrap()
            .is_empty(),
        "limit 0 must return no lexical hits"
    );
    assert!(
        store
            .dense_search(&bank_a, &near, &[], 0)
            .unwrap()
            .is_empty(),
        "limit 0 must return no dense hits"
    );

    // Coverage counts (bank-scoped).
    let cov = store.index_coverage(Some(&bank_a)).unwrap();
    assert_eq!(cov.bank_id, Some(bank_a.clone()));
    assert_eq!(cov.indexable_items, 1, "task item was invalidated");
    assert_eq!(cov.dense_rows, 1);
    assert_eq!(cov.lexical_rows, 1);
    let total = store.index_coverage(None).unwrap();
    assert!(
        total.indexable_items >= cov.indexable_items,
        "whole-store coverage is a superset of one bank"
    );

    // Queue lifecycle: failure bookkeeping + backoff cutoff + finish.
    store
        .fail_index_job(&bank_a, &id_1, "embed sidecar down")
        .unwrap();
    let cov2 = store.index_coverage(Some(&bank_a)).unwrap();
    assert_eq!(cov2.retry_jobs, 1);
    assert_eq!(cov2.last_error.as_deref(), Some("embed sidecar down"));
    let cov_total = store.index_coverage(None).unwrap();
    assert_eq!(
        cov_total.last_error.as_deref(),
        Some("embed sidecar down"),
        "whole-store scope surfaces the failure too"
    );
    let due = store
        .pending_index_jobs(ALL_PENDING, Some("0"), u32::MAX)
        .unwrap();
    assert!(
        due.iter()
            .all(|j| !(j.bank_id == bank_a && j.item_id == id_1)),
        "failed row must sit out the retry backoff window"
    );
    // A row at the retry cap leaves the due set but stays visible (#14): the
    // worker never retries it, so it must not take batch slots from newer rows.
    // id_1 already carries one failed attempt from the backoff checks above.
    let cap = 2_u32;
    store
        .fail_index_job(&bank_a, &id_1, "embed sidecar down")
        .unwrap();
    assert!(
        store
            .pending_index_jobs(ALL_PENDING, None, cap)
            .unwrap()
            .iter()
            .all(|j| !(j.bank_id == bank_a && j.item_id == id_1)),
        "a row at the retry cap must not be due"
    );
    assert!(
        store
            .pending_index_jobs(ALL_PENDING, None, cap + 1)
            .unwrap()
            .iter()
            .any(|j| j.bank_id == bank_a && j.item_id == id_1),
        "the capped row stays visible to backlog reporting"
    );
    assert!(
        store
            .pending_index_jobs(ALL_PENDING, None, u32::MAX)
            .unwrap()
            .iter()
            .any(|j| j.bank_id == bank_a && j.item_id == id_1),
        "backoff is lifted when the cutoff is None"
    );
    store.finish_index_job(&bank_a, &id_1).unwrap();
    store.finish_index_job(&bank_a, &id_2).unwrap();
    assert!(
        store
            .pending_index_jobs(ALL_PENDING, None, u32::MAX)
            .unwrap()
            .iter()
            .all(|j| j.bank_id != bank_a),
        "both bank rows are drained"
    );

    // Delete removes derived rows (removal policy on hard discard).
    assert!(store.delete_item(&bank_a, &id_1).unwrap());
    assert!(
        store
            .dense_search(&bank_a, &near, &[], 2)
            .unwrap()
            .is_empty(),
        "bank_a now holds no visible items: deleted + invalidated"
    );
    assert!(
        store
            .lexical_search(&bank_a, "ECONNRESET", &[], 5)
            .unwrap()
            .is_empty(),
        "lexical rows must go with the item"
    );
    // The deleted item's lexical row went with it; explicit cleanup idempotent.
    assert!(!store.delete_lexical_doc(&bank_a, &id_1).unwrap());
    assert!(store.delete_lexical_doc(&bank_a, &id_2).unwrap());
    assert!(!store.delete_lexical_doc(&bank_a, &id_2).unwrap());

    // Widening parity: domain filters under-return until the fetch widens or
    // candidates exhaust (no task items carry this token).
    let widen_1 = unique("widen-1");
    let widen_2 = unique("widen-2");
    for id in [&widen_1, &widen_2] {
        store
            .create_memory_item(
                &item(id, &bank_a, ItemKind::Semantic, "widening tok"),
                "actor",
            )
            .unwrap();
        store
            .put_lexical_doc(&bank_a, id, "widening tok tok")
            .unwrap();
        store
            .upsert_vector(&slot(id, &bank_a, &[0.4, 0.6]))
            .unwrap();
    }
    let widened_lex = store
        .lexical_search(&bank_a, "tok", &[SearchDomain::Task], 1)
        .unwrap();
    assert!(
        widened_lex.is_empty(),
        "no task item carries tok: {widened_lex:?}"
    );
    let widened_dense = store
        .dense_search(&bank_a, &near, &[SearchDomain::Task], 1)
        .unwrap();
    assert!(widened_dense.is_empty(), "no task item carries a vector");

    // Rebuild sweep support: banks, offset paging, orphan cleanup, validation.
    let banks = store.indexable_banks().unwrap();
    assert!(
        banks.iter().any(|b| b == &bank_a),
        "scope discovery finds the bank"
    );
    let paged = store.indexable_items(&bank_a, 1, 0).unwrap();
    assert_eq!(paged, vec![widen_1.clone()]);
    let offset_page = store.indexable_items(&bank_a, 1, 1).unwrap();
    assert_eq!(offset_page, vec![widen_2.clone()]);

    // Validation parity: empty banks/queries and missing targets fail closed.
    assert_eq!(
        store.lexical_search("", "x", &[], 5).unwrap_err().code,
        clio_types::ErrorCode::MissingBank
    );
    assert_eq!(
        store.lexical_search(&bank_a, "", &[], 5).unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        store.dense_search("", &near, &[], 5).unwrap_err().code,
        clio_types::ErrorCode::MissingBank
    );
    assert_eq!(
        store.enqueue_index_job("", "x").unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .put_lexical_doc(&bank_a, "missing-item", "x")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::NotFound
    );
    assert!(!store.delete_lexical_doc(&bank_a, "missing-item").unwrap());

    // Orphan cleanup: queue rows (and SQLite vector rows) for vanished items.
    // Bank-scoped cleanup only touches the requested bank's orphans.
    assert!(store.enqueue_index_job(&bank_a, "vanished-item-a").unwrap());
    assert!(store.enqueue_index_job(&bank_b, "vanished-item-b").unwrap());
    let removed_b = store.cleanup_orphans(Some(&bank_b)).unwrap();
    assert!(
        removed_b >= 1,
        "bank-scoped cleanup must remove that bank's orphans: {removed_b}"
    );
    assert!(
        store
            .pending_index_jobs(ALL_PENDING, None, u32::MAX)
            .unwrap()
            .iter()
            .any(|j| j.bank_id == bank_a && j.item_id == "vanished-item-a"),
        "scoped cleanup must not touch other banks' orphan rows"
    );
    let removed = store.cleanup_orphans(None).unwrap();
    assert!(
        removed >= 1,
        "whole-store cleanup must remove remaining orphans: {removed}"
    );
    assert!(
        store
            .pending_index_jobs(ALL_PENDING, None, u32::MAX)
            .unwrap()
            .iter()
            .all(|j| j.item_id != "vanished-item-a"),
        "whole-store cleanup removes the other bank's orphan too"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dek::{DekProvider, LocalDevKms};
    use crate::postgres::PostgresStore;
    use crate::sqlite::SqliteStore;
    use std::sync::Arc;

    #[test]
    fn sqlite_index_contract() {
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
        run_index_suite(&store);
    }

    #[test]
    fn postgres_index_contract() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = PostgresStore::open(url.as_str(), Arc::clone(&kms)).unwrap();
        run_index_suite(&store);
    }
}
