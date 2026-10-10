// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite active-embedding-space suite:
//! recorded-space resolution, width-parameterized vector DDL, the confirmed
//! space switch, and width fail-closed behavior.
//!
//! ## Owns
//! - SQLite space bootstrap, switch, and width-gate assertions
//!
//! ## Does not own
//! - Space identity policy (`embedding_space`) or DDL text (`vector_ddl`)
//! - Postgres space behavior (see `pg_space_tests`)
//!
//! ## Boundary
//!
//! Changes to SQLite space reconciliation should normally update the suite here.
//! Changes to shared width policy should not require changes here.
//!
//! Keep this module focused on SQLite space behavior.

use crate::dek::{DekProvider, LocalDevKms};
use crate::embedding_space::{EmbeddingSpace, SETTING_EMBEDDING_DIM, SETTING_EMBEDDING_MODEL};
use crate::index_store::IndexStore;
use crate::model::{EMBEDDING_DIMS, VectorSlot};
use crate::space_reconcile::SpaceReconcile;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use rusqlite::OptionalExtension;
use serde_json::json;
use std::sync::Arc;

fn store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).unwrap()
}

fn item(id: &str, bank: &str) -> MemoryItem {
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

fn slot(id: &str, bank: &str, dims: usize) -> VectorSlot {
    let mut embedding = vec![0.0_f32; dims];
    embedding[0] = 1.0;
    VectorSlot {
        item_id: id.into(),
        bank_id: bank.into(),
        model_id: "BAAI/bge-small-en-v1.5".into(),
        embedding,
    }
}

fn table_sql(store: &SqliteStore) -> String {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'item_embeddings'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string()))
        })
        .unwrap()
}

fn setting(store: &SqliteStore, key: &str) -> Option<String> {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT value FROM schema_settings WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| clio_types::AmError::new(clio_types::ErrorCode::Internal, e.to_string()))
        })
        .unwrap()
}

#[test]
fn fresh_store_records_the_default_space() {
    let store = store();
    let space = store.embedding_space().unwrap();
    assert_eq!(space, EmbeddingSpace::default_space());
    assert_eq!(
        setting(&store, SETTING_EMBEDDING_DIM).as_deref(),
        Some("384")
    );
    assert_eq!(
        setting(&store, SETTING_EMBEDDING_MODEL).as_deref(),
        Some("BAAI/bge-small-en-v1.5")
    );
    assert!(
        table_sql(&store).contains("float[384]"),
        "{}",
        table_sql(&store)
    );

    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
        .unwrap();
    let hits = store.knn("b1", &vec![1.0_f32; EMBEDDING_DIMS], 1).unwrap();
    assert_eq!(hits.first().expect("knn hit").item_id, "i1");
}

#[test]
fn fresh_store_adopts_a_configured_space_without_opt_in() {
    let mut store = store();
    let target = EmbeddingSpace::new("local/wide", 1536).unwrap();
    // Nothing to purge on a fresh database, so no destructive-switch opt-in.
    let report = store.apply_configured_space(&target, false).unwrap();
    assert!(report.switched);
    assert_eq!(report.purged_rows, 0);
    assert!(
        table_sql(&store).contains("float[1536]"),
        "{}",
        table_sql(&store)
    );
    assert_eq!(
        setting(&store, SETTING_EMBEDDING_DIM).as_deref(),
        Some("1536")
    );
    assert_eq!(
        setting(&store, SETTING_EMBEDDING_MODEL).as_deref(),
        Some("local/wide")
    );
    assert_eq!(store.embedding_space().unwrap(), target);
}

#[test]
fn foreign_model_id_is_refused_on_upsert() {
    let store = store();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    let err = store
        .upsert_vector(&VectorSlot {
            model_id: "openai/text-embedding-3-small".into(),
            ..slot("i1", "b1", EMBEDDING_DIMS)
        })
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert!(err.message.contains("active space model"), "{err}");
}

#[test]
fn equal_space_reconcile_is_a_noop() {
    let mut store = store();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
        .unwrap();
    let report = store
        .apply_configured_space(&EmbeddingSpace::default_space(), true)
        .unwrap();
    assert!(!report.switched);
    assert_eq!(report.purged_rows, 0);
    assert!(table_sql(&store).contains("float[384]"));
    assert_eq!(store.knn("b1", &vec![1.0_f32; 384], 1).unwrap().len(), 1);
}

#[test]
fn unconfirmed_switch_is_refused_and_keeps_rows() {
    let mut store = store();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
        .unwrap();
    let target = EmbeddingSpace::new("openai/text-embedding-3-small", 1536).unwrap();
    let err = store.apply_configured_space(&target, false).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("embed.switch_space=true"), "{err}");
    // Nothing was dropped: the old space still serves its rows.
    assert!(table_sql(&store).contains("float[384]"));
    assert_eq!(store.embedding_space().unwrap().dims(), 384);
    assert_eq!(store.knn("b1", &vec![1.0_f32; 384], 1).unwrap().len(), 1);
}

#[test]
fn confirmed_switch_recreates_the_table_at_the_new_width() {
    let mut store = store();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
        .unwrap();
    let target = EmbeddingSpace::new("local/nomic-embed-text", 768).unwrap();
    let report = store.apply_configured_space(&target, true).unwrap();
    assert!(report.switched);
    assert_eq!(report.purged_rows, 1);
    assert_eq!(report.from.dims(), 384);
    assert_eq!(report.to.dims(), 768);

    assert!(
        table_sql(&store).contains("float[768]"),
        "{}",
        table_sql(&store)
    );
    assert_eq!(
        setting(&store, SETTING_EMBEDDING_DIM).as_deref(),
        Some("768")
    );
    assert_eq!(
        setting(&store, SETTING_EMBEDDING_MODEL).as_deref(),
        Some("local/nomic-embed-text")
    );
    assert_eq!(store.embedding_space().unwrap(), target);

    // The re-embedded item works in the new space; the old width fails closed.
    store
        .upsert_vector(&VectorSlot {
            model_id: "local/nomic-embed-text".into(),
            ..slot("i1", "b1", 768)
        })
        .unwrap();
    assert_eq!(store.knn("b1", &vec![1.0_f32; 768], 1).unwrap().len(), 1);
    assert_eq!(
        store
            .knn("b1", &vec![1.0_f32; EMBEDDING_DIMS], 1)
            .unwrap_err()
            .code,
        clio_types::ErrorCode::OutOfRange
    );
    assert_eq!(
        store
            .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
            .unwrap_err()
            .code,
        clio_types::ErrorCode::OutOfRange
    );
}

#[test]
fn switch_back_leaves_no_mixed_rows() {
    let mut store = store();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
        .unwrap();
    let wide = EmbeddingSpace::new("local/wide", 1536).unwrap();
    store.apply_configured_space(&wide, true).unwrap();
    store
        .upsert_vector(&VectorSlot {
            model_id: "local/wide".into(),
            ..slot("i1", "b1", 1536)
        })
        .unwrap();

    let back = store
        .apply_configured_space(&EmbeddingSpace::default_space(), true)
        .unwrap();
    assert!(back.switched);
    assert_eq!(back.purged_rows, 1, "the wide-space row is discarded");
    assert!(table_sql(&store).contains("float[384]"));
    store
        .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
        .unwrap();
    assert_eq!(store.knn("b1", &vec![1.0_f32; 384], 1).unwrap().len(), 1);
}

#[test]
fn switch_preserves_every_non_vector_table() {
    let mut store = store();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .create_memory_item(&item("i2", "b1"), "actor")
        .unwrap();
    store
        .upsert_vector(&slot("i1", "b1", EMBEDDING_DIMS))
        .unwrap();
    let before_item = store.get_memory_item("b1", "i1").unwrap();
    let before_coverage = store.index_coverage(Some("b1")).unwrap();

    store
        .apply_configured_space(&EmbeddingSpace::new("m", 1536).unwrap(), true)
        .unwrap();

    let after_item = store.get_memory_item("b1", "i1").unwrap();
    assert_eq!(
        before_item, after_item,
        "item content survives a space switch"
    );
    let after_coverage = store.index_coverage(Some("b1")).unwrap();
    assert_eq!(
        before_coverage.indexable_items,
        after_coverage.indexable_items
    );
    assert_eq!(after_coverage.dense_rows, 0, "derived vectors were purged");
    assert_eq!(
        before_coverage.lexical_rows, after_coverage.lexical_rows,
        "lexical docs are untouched"
    );
    assert!(store.get_memory_item("b1", "i2").unwrap().is_some());
}

#[test]
fn recorded_space_is_read_from_settings_and_fails_closed_when_half_set() {
    let store = store();
    store
        .test_execute("DELETE FROM schema_settings WHERE key = 'embedding_dim'")
        .unwrap();
    let err = store
        .with_conn(|conn| crate::sqlite::read_space(conn))
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);

    store
        .test_execute("DELETE FROM schema_settings WHERE key = 'embedding_model'")
        .unwrap();
    let space = store
        .with_conn(|conn| crate::sqlite::read_space(conn))
        .unwrap();
    assert_eq!(
        space, None,
        "both rows absent means a fresh (unrecorded) space"
    );
}

#[test]
fn reconcile_seam_and_config_helpers() {
    let mut store = store();
    assert!(store.reconcile_space(None, false).unwrap().is_none());

    let target = EmbeddingSpace::new("local/nomic-embed-text", 768).unwrap();
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
    assert!(
        store
            .reconcile_space(Some(&EmbeddingSpace::new("m", 1536).unwrap()), false)
            .is_err()
    );

    let effective = serde_json::json!({
        "embed": {"model": "local/nomic-embed-text", "dims": 768, "switch_space": true}
    });
    assert_eq!(
        crate::configured_embedding_space(&effective).unwrap(),
        Some(target)
    );
    assert!(crate::space_switch_confirmed(&effective));
    assert!(!crate::space_switch_confirmed(&serde_json::json!({})));
    assert_eq!(
        crate::configured_embedding_space(&serde_json::json!({})).unwrap(),
        None
    );
}
