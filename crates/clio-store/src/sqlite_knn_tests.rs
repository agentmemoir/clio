// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite KNN visibility tests: a corrected
//! prior keeps its vector row but must never be returned, and the widening
//! fetch must reach the live revision behind it.
//!
//! ## Owns
//! - The closed-row exclusion and widening assertions for `Store::knn`
//!
//! ## Does not own
//! - Postgres execution parity (see `store_parity_tests`)
//! - The backend scan mechanics (see `vector_backend`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on SQLite KNN visibility.

use crate::model::{EMBEDDING_DIMS, VectorSlot};
use crate::test_unique::unique;
use crate::{CorrectionSpec, DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

fn item(id: &str, bank: &str, gist: &str) -> MemoryItem {
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
        source_ref: Some("knn-test".into()),
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

fn upsert(store: &SqliteStore, bank: &str, id: &str, embedding: Vec<f32>) {
    store
        .upsert_vector(&VectorSlot {
            item_id: id.into(),
            bank_id: bank.into(),
            model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
            embedding,
        })
        .expect("upsert vector");
}

#[test]
fn knn_widens_past_a_closed_row_to_the_live_revision() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("sqlite");
    let bank = unique("bank-knn");
    let prior_id = unique("prior");
    let live_id = unique("live");

    let prior = item(&prior_id, &bank, "old belief");
    store.create_memory_item(&prior, "agent").expect("prior");
    upsert(&store, &bank, &prior_id, unit_vector(0));

    let mut corrected = prior.clone();
    corrected.id = live_id.clone();
    corrected.gist = Some("current belief".into());
    corrected.tx_from = "2026-02-01T00:00:00Z".into();
    corrected.valid_from = "2026-02-01T00:00:00Z".into();
    corrected.created_at = "2026-02-01T00:00:00Z".into();
    corrected.updated_at = "2026-02-01T00:00:00Z".into();
    store
        .correct_item(&CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: prior_id.clone(),
            new_item: corrected,
            reason: "belief changed".into(),
            actor: "agent".into(),
            timestamp: "2026-02-01T00:00:00Z".into(),
        })
        .expect("correct");
    upsert(&store, &bank, &live_id, unit_vector(1));

    // The closed prior is the nearest vector: k = 1 must widen past it.
    let hits = store.knn(&bank, &unit_vector(0), 1).expect("knn");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].item_id, live_id, "{hits:?}");

    // Both vectors fit at k = 2: only the live revision is a candidate.
    let hits = store.knn(&bank, &unit_vector(0), 2).expect("knn");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].item_id, live_id, "{hits:?}");
}
