// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite item-row delete body: one transaction removes
//! the item row and every derived row SQLite does not cascade.
//!
//! ## Owns
//! - The delete-once/returns-false-twice assertion with vector, lexical, and
//!   outbox rows present
//!
//! ## Does not own
//! - Production behaviour
//! - The vector-table delete SQL (`vector_backend`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the SQLite item-row delete.

use crate::dek::{DekProvider, LocalDevKms};
use crate::index_store::IndexStore;
use crate::model::{EMBEDDING_DIMS, VectorSlot};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subject-{bank}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.6),
        source_ref: Some("envelope-test".into()),
        context: None,
        snapshot: Some(json!({"gist": "delete me"})),
        gist: Some("delete me".into()),
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

#[test]
fn delete_removes_the_item_row_and_every_derived_row_in_one_transaction() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-envelope");
    let id = unique("item");

    store
        .create_memory_item(&item(&id, &bank), "agent")
        .expect("create");
    store
        .put_lexical_doc(&bank, &id, "delete me once")
        .expect("lexical doc");
    store
        .upsert_vector(&VectorSlot {
            item_id: id.clone(),
            bank_id: bank.clone(),
            model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
            embedding: unit_vector(0),
        })
        .expect("vector row");

    assert!(store.delete_item(&bank, &id).expect("first delete"));

    assert_eq!(
        store
            .get_memory_item(&bank, &id)
            .expect("read after delete"),
        None,
        "the item row must be gone"
    );
    assert!(
        store
            .lexical_search(&bank, "delete", &[], 5)
            .expect("lexical after delete")
            .is_empty(),
        "the FTS row must be gone"
    );
    assert!(
        store
            .pending_index_jobs(1_000_000, None, u32::MAX)
            .expect("outbox after delete")
            .iter()
            .all(|job| job.item_id != id),
        "the outbox row must be gone"
    );
    assert!(
        !store
            .delete_vector(&bank, &id)
            .expect("vector after delete"),
        "the vector row must already be gone with the item"
    );
    assert!(
        store
            .knn(&bank, &unit_vector(0), 5)
            .expect("knn")
            .is_empty(),
        "no vector may remain to be scanned"
    );
    assert!(
        !store.delete_item(&bank, &id).expect("second delete"),
        "a repeated delete must report false"
    );
}

#[test]
fn deleting_an_unknown_item_reports_false_and_spares_the_rest() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-envelope");
    let kept = unique("kept");
    store
        .create_memory_item(&item(&kept, &bank), "agent")
        .expect("create");

    assert!(
        !store
            .delete_item(&bank, &unique("absent"))
            .expect("unknown delete"),
        "deleting an unknown id must report false"
    );
    assert!(
        store
            .get_memory_item(&bank, &kept)
            .expect("read the kept item")
            .is_some(),
        "an unrelated item must survive"
    );
}
