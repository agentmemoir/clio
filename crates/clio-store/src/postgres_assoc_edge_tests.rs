// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres association-edge coverage and the
//! cross-backend parity sample (identical weights and edge rows vs SQLite).
//!
//! ## Owns
//! - Live-Postgres fixtures for assoc CRUD/reinforce and dual-backend parity
//!
//! ## Does not own
//! - The pure weight formula (`assoc_policy_tests`) or SQLite-only edge cases
//!   (`assoc_edge_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Kept separate for the
//! 450-line file cap.

use crate::dek::{CiphertextEnvelope, DekProvider, LocalDevKms};
use crate::model::{AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, VectorSlot};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{AmError, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use std::sync::Arc;

fn postgres_url() -> String {
    crate::db_url::postgres_url_from_env()
}

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

fn sample_slot(id: &str) -> VectorSlot {
    VectorSlot {
        item_id: id.into(),
        bank_id: "b".into(),
        model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
        embedding: vec![0.0_f32; EMBEDDING_DIMS],
    }
}

#[test]
fn postgres_validation_empty_cipher() {
    let _guard = crate::lock_pg_test();
    let url = postgres_url();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = PostgresStore::open(&url, Arc::clone(&kms)).unwrap();
    let mut bad = sample_item("", "b");
    bad.subject_id = "s".into();
    assert_eq!(
        store.create_memory_item(&bad, "actor").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .put_assoc_edge(&AssocEdgeRecord {
                id: String::new(),
                bank_id: "b".into(),
                source_id: "a".into(),
                target_id: "c".into(),
                assoc_kind: AssocKind::Explicit,
                relationship: None,
                weight: 0.1,
                last_updated_at: "t".into(),
            })
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .put_assoc_edge(&AssocEdgeRecord {
                id: "e".into(),
                bank_id: "b".into(),
                source_id: "a".into(),
                target_id: "c".into(),
                assoc_kind: AssocKind::Explicit,
                relationship: Some("r".into()),
                weight: 1.5,
                last_updated_at: "t".into(),
            })
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );

    let id = format!("pg-empty-{}", std::process::id());
    store
        .create_memory_item(&sample_item(&id, "b"), "actor")
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = '' WHERE id = '{id}'"
        ))
        .unwrap();
    assert_eq!(
        store.get_memory_item("b", &id).unwrap_err().code,
        ErrorCode::UnreadableContent
    );
    store
        .upsert_subject_key_ref("s2", "local:s2:v1", "active")
        .unwrap();
    store
        .upsert_subject_key_ref("s2", "local:s2:v1", "shredded")
        .unwrap();
    let shredded_hits = store.knn("b", &vec![0.0_f32; EMBEDDING_DIMS], 0).unwrap();
    assert!(shredded_hits.is_empty(), "{shredded_hits:?}");
    assert!(
        store
            .inspect_content_ciphertext("missing", "nope")
            .unwrap()
            .is_none()
    );
}

#[test]
fn postgres_sql_errors_via_empty_schema() {
    let _guard = crate::lock_pg_test();
    let url = postgres_url();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = PostgresStore::open(&url, kms).unwrap();
    assert!(store.test_execute("THIS IS NOT SQL").is_err());

    let schema = format!("am_broken_{}", std::process::id());
    store
        .test_execute(&format!("CREATE SCHEMA {schema}"))
        .unwrap();
    store
        .test_execute(&format!("SET search_path TO {schema}"))
        .unwrap();
    assert!(
        store
            .create_memory_item(&sample_item("x", "b"), "actor")
            .is_err()
    );
    assert!(store.get_memory_item("b", "x").is_err());
    assert!(store.delete_item("b", "x").is_err());
    assert!(store.put_assoc_edge(&sample_edge("e")).is_err());
    assert!(store.get_assoc_edge("b", "e").is_err());
    assert!(store.delete_assoc_edge("b", "e").is_err());
    assert!(store.upsert_vector(&sample_slot("v")).is_err());
    assert!(store.delete_vector("b", "v").is_err());
    assert!(store.knn("b", &vec![0.0_f32; EMBEDDING_DIMS], 1).is_err());
    assert!(
        store
            .upsert_subject_key_ref("s", "local:s:v1", "active")
            .is_err()
    );
    assert!(store.inspect_content_ciphertext("b", "x").is_err());
    store.test_execute("SET search_path TO public").unwrap();
    store
        .test_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .unwrap();
}

#[test]
fn postgres_crypto_and_dim_errors() {
    let _guard = crate::lock_pg_test();
    let url = postgres_url();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = PostgresStore::open(&url, Arc::clone(&kms)).unwrap();
    let id = format!("pg-mal-{}", std::process::id());
    store
        .create_memory_item(&sample_item(&id, "b"), "actor")
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = '{{bad' WHERE id = '{id}'"
        ))
        .unwrap();
    assert!(store.get_memory_item("b", &id).is_err());
    assert!(
        store
            .upsert_vector(&VectorSlot {
                item_id: id,
                bank_id: "b".into(),
                model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
                embedding: vec![0.0; 2],
            })
            .is_err()
    );
    kms.destroy_dek("s").unwrap();
    assert!(
        store
            .create_memory_item(
                &sample_item(&format!("pg-dek-{}", std::process::id()), "b"),
                "actor"
            )
            .is_err()
    );
}

struct FailEncryptKms;

impl DekProvider for FailEncryptKms {
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError> {
        Ok(format!("local:{subject_id}:v1"))
    }
    fn encrypt(&self, _: &str, _: &[u8]) -> Result<CiphertextEnvelope, AmError> {
        Err(AmError::new(ErrorCode::Internal, "inject encrypt failure"))
    }
    fn decrypt(&self, _: &CiphertextEnvelope) -> Result<Vec<u8>, AmError> {
        Err(AmError::new(ErrorCode::Forbidden, "no"))
    }
    fn blind_index_key(&self) -> Result<[u8; 32], AmError> {
        Err(AmError::new(ErrorCode::Internal, "inject key failure"))
    }
    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError> {
        Err(AmError::new(ErrorCode::Internal, "inject key failure"))
    }
    fn destroy_dek(&self, _: &str) -> Result<(), AmError> {
        Ok(())
    }
    fn diagnose_masked(&self, subject_id: &str) -> String {
        format!("subject_id={subject_id} status=test")
    }
}

#[test]
fn create_memory_item_encrypt_failure_is_surfaced() {
    let _guard = crate::lock_pg_test();
    let store =
        SqliteStore::open_in_memory(Arc::new(FailEncryptKms) as Arc<dyn DekProvider>).unwrap();
    assert!(
        store
            .create_memory_item(&sample_item("enc-fail", "b"), "actor")
            .is_err()
    );
    let store = PostgresStore::open(
        &postgres_url(),
        Arc::new(FailEncryptKms) as Arc<dyn DekProvider>,
    )
    .unwrap();
    assert!(
        store
            .create_memory_item(&sample_item("enc-fail-pg", "b"), "actor")
            .is_err()
    );
}

#[test]
fn postgres_bad_assoc_kind_and_readonly_migrate() {
    let _guard = crate::lock_pg_test();
    let url = postgres_url();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = PostgresStore::open(&url, Arc::clone(&kms)).unwrap();
    store.put_assoc_edge(&sample_edge("pg-badk")).unwrap();
    store
        .test_execute("ALTER TABLE assoc_edges DROP CONSTRAINT assoc_edges_assoc_kind_check")
        .unwrap();
    store
        .test_execute("UPDATE assoc_edges SET assoc_kind = 'distilled' WHERE id = 'pg-badk'")
        .unwrap();
    assert!(store.get_assoc_edge("b", "pg-badk").is_err());
    // Restore constraint for later tests.
    store
        .test_execute("DELETE FROM assoc_edges WHERE id = 'pg-badk'")
        .unwrap();
    store
        .test_execute(
            "ALTER TABLE assoc_edges ADD CONSTRAINT assoc_edges_assoc_kind_check \
             CHECK (assoc_kind = ANY (ARRAY['coactivation'::text, 'explicit'::text]))",
        )
        .unwrap();

    let separator = if url.contains('?') { '&' } else { '?' };
    let ro = format!("{url}{separator}options=-c%20default_transaction_read_only%3Don");
    drop(
        PostgresStore::open(&ro, Arc::clone(&kms))
            .expect("a current schema needs no migration writes"),
    );
    store
        .test_execute("DELETE FROM schema_settings WHERE key = 'postgres_bootstrap_version'")
        .expect("make bootstrap migration necessary");
    let refused = PostgresStore::open(&ro, Arc::clone(&kms));
    drop(PostgresStore::open(&url, kms).expect("restore the bootstrap stamp with a writable open"));
    let Err(error) = refused else {
        panic!("a required migration must fail on a read-only connection")
    };
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("read-only transaction"),
        "the migration must fail because writes are refused: {error}"
    );
}

#[test]
fn t14_dual_backend_reinforce_parity() {
    let _guard = crate::lock_pg_test();
    let bank = format!("parity-{}", std::process::id());
    let sqlite =
        SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>).unwrap();
    let pg = PostgresStore::open(
        &postgres_url(),
        Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>,
    )
    .unwrap();

    let mut sqlite_w = 0.0;
    let mut pg_w = 0.0;
    for _ in 0..3 {
        sqlite_w = sqlite
            .reinforce_assoc_edge(&bank, "x", "y", 0, 0.15)
            .unwrap()
            .weight;
        pg_w = pg
            .reinforce_assoc_edge(&bank, "x", "y", 0, 0.15)
            .unwrap()
            .weight;
    }
    assert!((sqlite_w - pg_w).abs() < 1e-12, "{sqlite_w} != {pg_w}");
    assert!((sqlite_w - 0.385_875).abs() < 1e-12);

    for rel in ["caused_by", "depends_on"] {
        let edge = AssocEdgeRecord {
            id: crate::assoc::explicit_id(&bank, "x", "y", rel),
            bank_id: bank.clone(),
            source_id: "x".into(),
            target_id: "y".into(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some(rel.into()),
            weight: 0.7,
            last_updated_at: "0".into(),
        };
        sqlite.put_assoc_edge(&edge).unwrap();
        pg.put_assoc_edge(&edge).unwrap();
    }
    let mut s = sqlite.list_assoc_edges(&bank, Some("x")).unwrap();
    let mut p = pg.list_assoc_edges(&bank, Some("x")).unwrap();
    assert_eq!(s.len(), 3);
    assert_eq!(p.len(), 3);
    s.sort_by(|a, b| a.id.cmp(&b.id));
    p.sort_by(|a, b| a.id.cmp(&b.id));
    for (a, b) in s.iter().zip(p.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.assoc_kind, b.assoc_kind);
        assert_eq!(a.relationship, b.relationship);
        assert!((a.weight - b.weight).abs() < 1e-12);
    }

    // reinforce validation branches + whole-bank list on the PG backend.
    assert_eq!(
        pg.reinforce_assoc_edge("", "x", "y", 0, 0.15)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        pg.reinforce_assoc_edge(&bank, "x", "x", 0, 0.15)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        pg.reinforce_assoc_edge(&bank, "x", "y", 0, 2.0)
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(pg.list_assoc_edges(&bank, None).unwrap().len(), 3);

    pg.record_assoc_audit(
        &bank,
        "graph_link",
        Some("x"),
        "agent",
        &serde_json::json!({"edge_id": "e"}),
    )
    .unwrap();
    assert_eq!(
        pg.record_assoc_audit("", "x", None, "agent", &serde_json::json!({}))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );

    sqlite
        .test_execute(&format!("DELETE FROM assoc_edges WHERE bank_id = '{bank}'"))
        .unwrap();
    pg.test_execute(&format!("DELETE FROM assoc_edges WHERE bank_id = '{bank}'"))
        .unwrap();
    // audit_events is append-only by design: rows for this
    // unique bank stay as audit history and are never cleaned up.
}
