// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared behavioral parity suite of the
//! operations-removal (`discard`) store path across both backends.
//!
//! ## Owns
//! - mutation, not-found, cross-bank,
//!   repeat policy, content-intact, bank isolation, and backend parity
//! - discarded items drop out of the default index/search visibility
//! - Episodic-memory discard lifecycle (NULL category) and active-only identity reads
//!
//! ## Does not own
//! - Tool-level confirmation gating and telemetry (see `clio-write` tests)
//! - Backend-specific discard SQL (`sqlite_discard` / `postgres_discard`)
//!
//! ## Boundary
//!
//! Changes to the `Store` discard contract should normally update the suite here.
//! Changes to one backend's SQL should not require changes here.
//!
//! Keep this module focused on backend-agnostic discard contract parity.

use crate::index_store::IndexStore;
use crate::model::{EMBEDDING_DIMS, VectorSlot};
use crate::store::Store;
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;

/// Combined trait object so one arg exercises both store contracts.
pub trait DiscardBackend: Store + IndexStore {
    /// Raw `(discarded_at, discard_reason)` pair for one item.
    fn raw_discard(&self, item_id: &str) -> (Option<String>, Option<String>);
}

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
        snapshot: Some(json!({"port": 8080})),
        gist: Some("service listens on 8080".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

/// Episodic items carry `category = NULL` in the schema.
fn sample_episodic(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: Some("turn_9".into()),
        snapshot: Some(json!({"subject": "svc", "predicate": "started", "object": "8080"})),
        gist: Some("svc started 8080".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn slot(id: &str, bank: &str) -> VectorSlot {
    let mut embedding = vec![0.0_f32; EMBEDDING_DIMS];
    embedding[0] = 1.0;
    VectorSlot {
        item_id: id.into(),
        bank_id: bank.into(),
        model_id: "BAAI/bge-small-en-v1.5".into(),
        embedding,
    }
}

/// Discard mutation contract for one backend.
fn suite_discard(store: &dyn DiscardBackend) {
    let bank_a = unique("db-a");
    let bank_b = unique("db-b");
    let id = unique("item");
    store
        .create_memory_item(&sample_semantic(&id, &bank_a), "actor")
        .unwrap();
    let cipher_before = store
        .inspect_content_ciphertext(&bank_a, &id)
        .unwrap()
        .unwrap();

    let identity = store
        .get_item_identity(&bank_a, &id)
        .unwrap()
        .expect("identity");
    assert_eq!(identity.item_id, id);
    assert_eq!(identity.category.as_deref(), Some("tool_config"));
    assert_eq!(identity.epistemic_kind, "fact");

    // Unknown id fails closed before any write.
    assert_eq!(
        store
            .discard_item(
                &bank_a,
                "missing-item",
                "ops cleanup",
                "2024-06-01",
                "agent"
            )
            .unwrap_err()
            .code,
        ErrorCode::ItemNotFound
    );
    // Cross-bank id is not found and never leaks existence.
    assert_eq!(
        store
            .discard_item(&bank_b, &id, "ops cleanup", "2024-06-01", "agent")
            .unwrap_err()
            .code,
        ErrorCode::ItemNotFound
    );
    assert!(store.get_item_identity(&bank_b, &id).unwrap().is_none());
    assert_eq!(
        store.raw_discard(&id),
        (None, None),
        "cross-bank wrote nothing"
    );

    // Input validation on the store primitive.
    assert_eq!(
        store
            .discard_item("", &id, "r", "2024-06-01", "agent")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .discard_item(&bank_a, "", "r", "2024-06-01", "agent")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .discard_item(&bank_a, &id, "", "2024-06-01", "agent")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store.raw_discard(&id),
        (None, None),
        "rejected input wrote nothing"
    );

    // Confirmed mutation sets both columns.
    let removed = store
        .discard_item(&bank_a, &id, "ops cleanup", "2024-06-01", "agent")
        .unwrap();
    assert_eq!(removed, identity);
    assert_eq!(
        store.raw_discard(&id),
        (Some("2024-06-01".into()), Some("ops cleanup".into()))
    );

    // Content payload and ciphertext are untouched by discard.
    let item = store.get_memory_item(&bank_a, &id).unwrap().unwrap();
    assert_eq!(item.snapshot, Some(json!({"port": 8080})));
    assert_eq!(item.gist.as_deref(), Some("service listens on 8080"));
    assert_eq!(
        store
            .inspect_content_ciphertext(&bank_a, &id)
            .unwrap()
            .unwrap(),
        cipher_before
    );

    // Repeat discard is rejected and leaves the first record intact.
    let err = store
        .discard_item(&bank_a, &id, "second attempt", "2024-06-02", "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("already discarded"));
    assert_eq!(
        store.raw_discard(&id),
        (Some("2024-06-01".into()), Some("ops cleanup".into()))
    );
}

/// Episodic-memory discard lifecycle (category is NULL end to end), and the
/// identity read only resolves active items.
fn suite_episodic_discard(store: &dyn DiscardBackend) {
    let bank = unique("ep-b");
    let id = unique("ep-item");
    store
        .create_memory_item(&sample_episodic(&id, &bank), "actor")
        .unwrap();

    let identity = store
        .get_item_identity(&bank, &id)
        .unwrap()
        .expect("episodic identity");
    assert_eq!(identity.item_id, id);
    assert_eq!(identity.category, None, "episodic category must be NULL");
    assert_eq!(identity.epistemic_kind, "fact");

    let removed = store
        .discard_item(&bank, &id, "ops cleanup", "2024-06-01", "agent")
        .unwrap();
    assert_eq!(removed, identity);
    assert_eq!(
        store.raw_discard(&id),
        (Some("2024-06-01".into()), Some("ops cleanup".into()))
    );
    // A discarded item no longer resolves an identity for the dry-run path.
    assert!(store.get_item_identity(&bank, &id).unwrap().is_none());
}

/// A discarded item drops out of default index/search visibility.
fn suite_visibility(store: &dyn DiscardBackend) {
    let bank = unique("vis-b");
    let id = unique("vis-item");
    store
        .create_memory_item(&sample_semantic(&id, &bank), "actor")
        .unwrap();
    store
        .put_lexical_doc(&bank, &id, "service listens on 8080")
        .unwrap();
    store.upsert_vector(&slot(&id, &bank)).unwrap();

    assert!(
        store
            .lexical_search(&bank, "8080", &[], 5)
            .unwrap()
            .iter()
            .any(|h| h.item_id == id),
        "item is visible before discard"
    );
    assert!(
        store
            .dense_search(&bank, &slot(&id, &bank).embedding, &[], 5)
            .unwrap()
            .iter()
            .any(|h| h.item_id == id),
        "item is densely visible before discard"
    );
    assert!(store.indexable_items(&bank, 10, 0).unwrap().contains(&id));
    assert_eq!(store.count_indexable_items(&bank).unwrap(), 1);

    store
        .discard_item(&bank, &id, "ops cleanup", "2024-06-01", "agent")
        .unwrap();

    assert!(
        store
            .lexical_search(&bank, "8080", &[], 5)
            .unwrap()
            .is_empty(),
        "discarded item must not be lexically searchable"
    );
    assert!(
        store
            .dense_search(&bank, &slot(&id, &bank).embedding, &[], 5)
            .unwrap()
            .is_empty(),
        "discarded item must not be densely searchable"
    );
    let indexable = store.indexable_items(&bank, 10, 0).unwrap();
    assert!(indexable.is_empty(), "{indexable:?}");
    assert_eq!(store.count_indexable_items(&bank).unwrap(), 0);
    assert_eq!(
        store.index_coverage(Some(&bank)).unwrap().indexable_items,
        0
    );
    // The row survives: discard is removal from active use, not deletion.
    assert!(store.get_memory_item(&bank, &id).unwrap().is_some());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dek::{DekProvider, LocalDevKms};
    use crate::postgres::PostgresStore;
    use crate::sqlite::SqliteStore;
    use rusqlite::{OptionalExtension, params};
    use std::sync::Arc;

    impl DiscardBackend for SqliteStore {
        fn raw_discard(&self, item_id: &str) -> (Option<String>, Option<String>) {
            self.with_conn(|conn| {
                conn.query_row(
                    "SELECT discarded_at, discard_reason FROM items WHERE id = ?1",
                    params![item_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))
            })
            .unwrap()
            .unwrap_or((None, None))
        }
    }

    impl DiscardBackend for PostgresStore {
        fn raw_discard(&self, item_id: &str) -> (Option<String>, Option<String>) {
            self.with_client(|client| {
                let row = client
                    .query_opt(
                        "SELECT discarded_at, discard_reason FROM items WHERE id = $1",
                        &[&item_id],
                    )
                    .map_err(|e| clio_types::AmError::new(ErrorCode::Internal, e.to_string()))?;
                Ok(row.map(|r| (r.get(0), r.get(1))))
            })
            .unwrap()
            .unwrap_or((None, None))
        }
    }

    #[test]
    fn sqlite_discard_contract() {
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = SqliteStore::open_in_memory(kms).unwrap();
        suite_discard(&store);
        suite_visibility(&store);
        suite_episodic_discard(&store);
    }

    #[test]
    fn postgres_discard_contract() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = PostgresStore::open(url.as_str(), kms).unwrap();
        suite_discard(&store);
        suite_visibility(&store);
        suite_episodic_discard(&store);
    }
}
