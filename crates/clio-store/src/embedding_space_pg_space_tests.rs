// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres active-embedding-space suite:
//! bootstrap, the confirmed space switch, and the fail-closed width/model gates.
//!
//! ## Owns
//! - Postgres space bootstrap/switch assertions
//!
//! ## Does not own
//! - Space identity policy (`embedding_space`) or DDL text (`vector_ddl`)
//! - The width → index-mode calibration and recall matrix (see `pg_calibration_tests`)
//! - SQLite space behavior (see `sqlite_space_tests`)
//!
//! ## Boundary
//!
//! Changes to Postgres space reconciliation should normally update the suite
//! here. Changes to the calibration matrix should not.
//!
//! Every case runs inside a private schema (`pg_scoped_schema`) so the shared
//! Compose database is never disturbed, and the schema is dropped when the case
//! finishes.

use crate::embedding_space::{EmbeddingSpace, SETTING_EMBEDDING_DIM};
use crate::model::{EMBEDDING_DIMS, VectorSlot};
use crate::pg_scoped_schema::ScopedSchema;
use crate::space_reconcile::SpaceReconcile;
use crate::store::Store;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;

pub(crate) fn base_url() -> String {
    crate::db_url::postgres_url_from_env()
}

pub(crate) fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_1".into()),
        snapshot: Some(json!({"service": "deploy-bot"})),
        gist: Some("deploy bot crashed".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

/// Deterministic pseudo-random unit-ish vector (no RNG dependency, stable across runs).
#[allow(clippy::cast_precision_loss)]
pub(crate) fn pseudo_vector(seed: usize, dims: usize) -> Vec<f32> {
    let mut state = (seed as u64)
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    let mut out = Vec::with_capacity(dims);
    for _ in 0..dims {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let unit = ((state >> 33) as u32) as f32 / u32::MAX as f32;
        out.push(unit * 2.0 - 1.0);
    }
    out
}

pub(crate) fn slot(id: &str, bank: &str, model: &str, embedding: Vec<f32>) -> VectorSlot {
    VectorSlot {
        item_id: id.into(),
        bank_id: bank.into(),
        model_id: model.into(),
        embedding,
    }
}

#[test]
fn postgres_space_roundtrip_keeps_non_vector_data() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let mut store = schema.open();
    assert_eq!(
        store.embedding_space().unwrap(),
        EmbeddingSpace::default_space()
    );
    assert_eq!(store.embedding_space().unwrap().dims(), EMBEDDING_DIMS);

    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .upsert_vector(&slot(
            "i1",
            "b1",
            "BAAI/bge-small-en-v1.5",
            pseudo_vector(1, EMBEDDING_DIMS),
        ))
        .unwrap();
    assert_eq!(
        store
            .knn("b1", &pseudo_vector(1, EMBEDDING_DIMS), 1)
            .unwrap()
            .len(),
        1
    );

    // A foreign model id is refused on write: one active space per deployment.
    let foreign = slot(
        "i1",
        "b1",
        "openai/text-embedding-3-small",
        pseudo_vector(1, EMBEDDING_DIMS),
    );
    assert_eq!(
        store.upsert_vector(&foreign).unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );

    let target = EmbeddingSpace::new("local/nomic-embed-text", 768).unwrap();
    let err = store.apply_configured_space(&target, false).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert_eq!(
        store
            .knn("b1", &pseudo_vector(1, EMBEDDING_DIMS), 1)
            .unwrap()
            .len(),
        1
    );

    let report = store.apply_configured_space(&target, true).unwrap();
    assert!(report.switched);
    assert_eq!(report.purged_rows, 1);
    let column: String = store
        .with_client(|client| {
            let row = client
                .query_one(
                    "SELECT format_type(atttypid, atttypmod) FROM pg_attribute
                     WHERE attrelid = 'item_embeddings'::regclass AND attname = 'embedding'",
                    &[],
                )
                .unwrap();
            Ok::<_, clio_types::AmError>(row.get(0))
        })
        .unwrap();
    assert_eq!(column, "vector(768)");
    assert!(store.get_memory_item("b1", "i1").unwrap().is_some());

    store
        .upsert_vector(&slot(
            "i1",
            "b1",
            "local/nomic-embed-text",
            pseudo_vector(1, 768),
        ))
        .unwrap();
    assert_eq!(store.knn("b1", &pseudo_vector(1, 768), 1).unwrap().len(), 1);
    assert_eq!(
        store
            .knn("b1", &pseudo_vector(1, EMBEDDING_DIMS), 1)
            .unwrap_err()
            .code,
        clio_types::ErrorCode::OutOfRange
    );

    let back = store
        .apply_configured_space(&EmbeddingSpace::default_space(), true)
        .unwrap();
    assert!(back.switched);
    assert_eq!(back.purged_rows, 1);
    assert!(store.get_memory_item("b1", "i1").unwrap().is_some());
}

#[test]
fn postgres_fresh_schema_adopts_a_configured_space_without_opt_in() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let mut store = schema.open();
    let target = EmbeddingSpace::new("local/wide", 768).unwrap();
    // A fresh schema has no derived rows, so no destructive-switch opt-in.
    let report = store.apply_configured_space(&target, false).unwrap();
    assert!(report.switched);
    assert_eq!(report.purged_rows, 0);
    assert_eq!(store.embedding_space().unwrap(), target);
}

#[test]
fn postgres_half_recorded_space_fails_closed() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let store = schema.open();
    store
        .test_execute(&format!(
            "DELETE FROM schema_settings WHERE key = '{SETTING_EMBEDDING_DIM}'"
        ))
        .unwrap();
    let err = store.with_client(crate::postgres::read_space).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
}

#[test]
fn postgres_reconcile_seam_arms() {
    let _guard = crate::lock_pg_test();
    let schema = ScopedSchema::new(&base_url());
    let mut store = schema.open();
    assert!(store.reconcile_space(None, false).unwrap().is_none());
    let target = EmbeddingSpace::new("calibration/model", 768).unwrap();
    let report = store
        .reconcile_space(Some(&target), true)
        .unwrap()
        .expect("switch report");
    assert!(report.switched);
    assert!(
        store
            .reconcile_space(Some(&target), false)
            .unwrap()
            .is_some()
    );
}
