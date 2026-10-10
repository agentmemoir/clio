// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite association-edge store edge cases:
//! CRUD round-trips, reinforce upsert math, concurrency, audit writes, and
//! bank isolation.
//!
//! ## Owns
//! - SQLite-focused fixtures for the `Store` association-edge surface
//!
//! ## Does not own
//! - The pure weight formula (`assoc_policy_tests`) or Postgres parity
//!   (`pg_assoc_edge_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::dek::{DekProvider, LocalDevKms};
use crate::model::{AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, VectorSlot};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use std::sync::Arc;

fn sample_item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "s".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.0),
        source_ref: Some("turn_9".into()),
        context: None,
        snapshot: Some(serde_json::json!({"id": id})),
        gist: Some("fixture item".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
    }
}

fn sample_edge(id: &str) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: "b".into(),
        source_id: "a".into(),
        target_id: "c".into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("r".into()),
        weight: 0.5,
        last_updated_at: "t".into(),
    }
}

#[test]
fn sqlite_empty_ciphertext_and_parent_create_fail() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    store
        .create_memory_item(&sample_item("i1", "b"), "actor")
        .unwrap();
    store
        .test_execute("UPDATE items SET content_ciphertext = '' WHERE id = 'i1'")
        .unwrap();
    assert_eq!(
        store.get_memory_item("b", "i1").unwrap_err().code,
        ErrorCode::UnreadableContent
    );

    let base = std::env::temp_dir().join(format!("am-blocked-{}", std::process::id()));
    std::fs::write(&base, b"not-a-dir").unwrap();
    let child = base.join("child.db");
    assert!(SqliteStore::open(&child, kms).is_err());
    let _ = std::fs::remove_file(&base);
}

#[test]
fn sqlite_knn_filters_other_bank_and_active_key_ref() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    for (id, bank) in [("a1", "bank-a"), ("b1", "bank-b")] {
        store
            .create_memory_item(&sample_item(id, bank), "actor")
            .unwrap();
        let mut emb = vec![0.0_f32; EMBEDDING_DIMS];
        emb[0] = if id == "a1" { 1.0 } else { 0.99 };
        store
            .upsert_vector(&VectorSlot {
                item_id: id.into(),
                bank_id: bank.into(),
                model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
                embedding: emb,
            })
            .unwrap();
    }
    let mut q = vec![0.0_f32; EMBEDDING_DIMS];
    q[0] = 1.0;
    let no_hits = store.knn("bank-a", &q, 0).unwrap();
    assert!(no_hits.is_empty(), "{no_hits:?}");
    let hits = store.knn("bank-a", &q, 2).unwrap();
    assert!(hits.iter().all(|h| h.item_id == "a1"));
    store
        .upsert_subject_key_ref("s", "local:s:v1", "active")
        .unwrap();
    store
        .upsert_subject_key_ref("s", "local:s:v1", "shredded")
        .unwrap();
    assert!(
        store
            .inspect_content_ciphertext("missing", "nope")
            .unwrap()
            .is_none()
    );
}

#[test]
fn sqlite_knn_finds_in_bank_despite_many_nearer_foreign_neighbors() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    // 40 nearer vectors in bank-b; 2 farther in bank-a. Fixed over-fetch would miss bank-a.
    for i in 0..40 {
        let id = format!("b{i}");
        store
            .create_memory_item(&sample_item(&id, "bank-b"), "actor")
            .unwrap();
        let mut emb = vec![0.0_f32; EMBEDDING_DIMS];
        emb[0] = 1.0;
        emb[1] = f32::from(u8::try_from(i).unwrap_or(255)) * 0.0001;
        store
            .upsert_vector(&VectorSlot {
                item_id: id,
                bank_id: "bank-b".into(),
                model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
                embedding: emb,
            })
            .unwrap();
    }
    for (id, skew) in [("a1", 0.05_f32), ("a2", 0.06_f32)] {
        store
            .create_memory_item(&sample_item(id, "bank-a"), "actor")
            .unwrap();
        let mut emb = vec![0.0_f32; EMBEDDING_DIMS];
        emb[0] = 1.0 - skew;
        store
            .upsert_vector(&VectorSlot {
                item_id: id.into(),
                bank_id: "bank-a".into(),
                model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
                embedding: emb,
            })
            .unwrap();
    }
    let mut q = vec![0.0_f32; EMBEDDING_DIMS];
    q[0] = 1.0;
    let hits = store.knn("bank-a", &q, 2).unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.item_id.starts_with('a')));
}

#[test]
fn sqlite_sql_errors_after_drop() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    store.test_execute("DROP TABLE items").unwrap();
    assert!(
        store
            .create_memory_item(&sample_item("x", "b"), "actor")
            .is_err()
    );
    assert!(store.get_memory_item("b", "x").is_err());
    assert!(store.delete_item("b", "x").is_err());
    assert!(store.inspect_content_ciphertext("b", "x").is_err());

    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap();
    store.test_execute("DROP TABLE assoc_edges").unwrap();
    assert!(store.put_assoc_edge(&sample_edge("e")).is_err());
    assert!(store.get_assoc_edge("b", "e").is_err());
    assert!(store.delete_assoc_edge("b", "e").is_err());

    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap();
    store.test_execute("DROP TABLE item_embeddings").unwrap();
    assert!(
        store
            .upsert_vector(&VectorSlot {
                item_id: "v".into(),
                bank_id: "b".into(),
                model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
                embedding: vec![0.0_f32; EMBEDDING_DIMS],
            })
            .is_err()
    );
    assert!(store.delete_vector("b", "v").is_err());
    assert!(store.knn("b", &vec![0.0_f32; EMBEDDING_DIMS], 1).is_err());

    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap();
    store.test_execute("DROP TABLE subject_keys").unwrap();
    assert!(
        store
            .create_memory_item(&sample_item("no-sk", "b"), "actor")
            .is_err()
    );
    assert!(
        store
            .upsert_subject_key_ref("s", "local:s:v1", "active")
            .is_err()
    );
}

#[test]
fn sqlite_crypto_and_dim_errors() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    store
        .create_memory_item(&sample_item("c1", "b"), "actor")
        .unwrap();
    kms.destroy_dek("s").unwrap();
    assert!(
        store
            .create_memory_item(&sample_item("c2", "b"), "actor")
            .is_err()
    );

    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    store
        .create_memory_item(&sample_item("m1", "b"), "actor")
        .unwrap();
    store
        .test_execute("UPDATE items SET content_ciphertext = '{not-json' WHERE id = 'm1'")
        .unwrap();
    assert!(store.get_memory_item("b", "m1").is_err());

    assert!(
        store
            .upsert_vector(&VectorSlot {
                item_id: "v".into(),
                bank_id: "b".into(),
                model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
                embedding: vec![0.0; 3],
            })
            .is_err()
    );

    let junk = std::env::temp_dir().join(format!("am-junk-{}.db", std::process::id()));
    std::fs::write(&junk, b"not-a-sqlite-database").unwrap();
    assert!(SqliteStore::open(&junk, Arc::clone(&kms)).is_err());
    let _ = std::fs::remove_file(&junk);
}

#[test]
fn sqlite_reinforce_list_and_bank_isolation() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let mut last = None;
    for _ in 0..3 {
        last = Some(store.reinforce_assoc_edge("b", "A", "B", 0, 0.15).unwrap());
    }
    let edge = last.unwrap();
    assert_eq!(edge.assoc_kind, AssocKind::Coactivation);
    assert_eq!(edge.relationship, None);
    assert!((edge.weight - 0.385_875).abs() < 1e-12, "{}", edge.weight);
    assert_eq!(edge.source_id, "A");
    assert_eq!(edge.target_id, "B");
    // Order-invariance: reversed call hits the same canonical row.
    store.reinforce_assoc_edge("b", "B", "A", 0, 0.15).unwrap();
    let by_a = store.list_assoc_edges("b", Some("A")).unwrap();
    let by_b = store.list_assoc_edges("b", Some("B")).unwrap();
    assert_eq!(by_a.len(), 1);
    assert_eq!(by_b.len(), 1);
    assert_eq!(by_a[0].id, by_b[0].id);
    assert_eq!(store.list_assoc_edges("b", None).unwrap().len(), 1);
    let other_bank_edges = store.list_assoc_edges("other-bank", None).unwrap();
    assert!(other_bank_edges.is_empty(), "{other_bank_edges:?}");
    // Validation.
    assert_eq!(
        store
            .reinforce_assoc_edge("b", "A", "A", 0, 0.15)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .reinforce_assoc_edge("", "A", "B", 0, 0.15)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .reinforce_assoc_edge("b", "A", "B", 0, 2.0)
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
}

#[test]
fn sqlite_concurrent_reinforce_keeps_every_increment() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = Arc::new(SqliteStore::open_in_memory(kms).unwrap());
    let threads = 4_u64;
    let per = 25_u64;
    let mut handles = Vec::new();
    for _ in 0..threads {
        let s = Arc::clone(&store);
        handles.push(std::thread::spawn(move || {
            for _ in 0..per {
                s.reinforce_assoc_edge("b", "A", "B", 0, 0.15).unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let mut expected = 0.0;
    for _ in 0..(threads * per) {
        expected = crate::assoc::grow(expected, 0.15);
    }
    let edges = store.list_assoc_edges("b", None).unwrap();
    assert_eq!(edges.len(), 1, "concurrent upserts must not duplicate rows");
    assert!(
        (edges[0].weight - expected).abs() < 1e-9,
        "{} != {expected}",
        edges[0].weight
    );
}

#[test]
fn sqlite_coactivation_pair_unique_index_blocks_duplicates() {
    // A second coactivation row for the same unordered pair must be rejected by
    // the partial unique index even when it carries a different edge id.
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let edge = |id: &str| AssocEdgeRecord {
        id: id.into(),
        bank_id: "b".into(),
        source_id: "A".into(),
        target_id: "B".into(),
        assoc_kind: AssocKind::Coactivation,
        relationship: None,
        weight: 0.1,
        last_updated_at: "0".into(),
    };
    store.put_assoc_edge(&edge("c1")).unwrap();
    assert!(store.put_assoc_edge(&edge("c2")).is_err());
}

#[test]
fn sqlite_list_assoc_edges_surfaces_sql_errors() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    store.test_execute("DROP TABLE assoc_edges").unwrap();
    assert!(store.list_assoc_edges("b", Some("A")).is_err());
    assert!(store.list_assoc_edges("b", None).is_err());
    assert!(store.reinforce_assoc_edge("b", "A", "B", 0, 0.15).is_err());
}

#[test]
fn sqlite_assoc_audit_validates_and_writes() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    assert_eq!(
        store
            .record_assoc_audit("", "assoc_prune", None, "agent", &serde_json::json!({}))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    store
        .record_assoc_audit(
            "b",
            "assoc_prune",
            Some("edge-1"),
            "agent",
            &serde_json::json!({"effective_weight": 0.01}),
        )
        .unwrap();
    store.test_execute("DROP TABLE audit_events").unwrap();
    assert!(
        store
            .record_assoc_audit(
                "b",
                "graph_link",
                Some("e"),
                "agent",
                &serde_json::json!({})
            )
            .is_err()
    );
}

#[test]
fn bad_assoc_kind_parse_on_read() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).unwrap();
    store.put_assoc_edge(&sample_edge("badk")).unwrap();
    store
        .test_execute(
            "CREATE TABLE assoc_edges_noccheck AS SELECT * FROM assoc_edges;
             DROP TABLE assoc_edges;
             ALTER TABLE assoc_edges_noccheck RENAME TO assoc_edges;
             UPDATE assoc_edges SET assoc_kind = 'distilled' WHERE id = 'badk';",
        )
        .unwrap();
    assert!(store.get_assoc_edge("b", "badk").is_err());
}
