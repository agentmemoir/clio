// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the opaque item persistence shred-guard
//! tests: an opaque `ciphertext_backup` import must not upsert an item for a
//! crypto-shredded subject.
//!
//! ## Owns
//! - The `PutOpaqueItem` erased-subject repro (SQLite + Postgres)
//!
//! ## Does not own
//! - The shred guard SQL and named error (`item_persist`)
//! - Opaque row persistence itself (`opaque_item_persist`)
//!
//! ## Boundary
//!
//! Changes to the opaque write shred guard should normally update tests here.
//! Changes to the ordinary item write path should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::compliance_store::ComplianceStore;
use crate::erase_test_support::{local_kms, memory_item, spec, unique};
use crate::model::OpaqueItemRecord;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{AmError, ErrorCode};

fn opaque_item(bank: &str, id: &str, subject: &str) -> OpaqueItemRecord {
    OpaqueItemRecord {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: "episodic".into(),
        category: None,
        episodic_type: Some("temporal".into()),
        epistemic_kind: "fact".into(),
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        content_cipher: "opaque-cipher".into(),
    }
}

#[test]
fn sqlite_put_opaque_item_refuses_erased_subject() {
    let store = SqliteStore::open_in_memory(local_kms()).expect("sqlite");
    let bank = unique("opq-b");
    let subject = unique("opq-s");
    store
        .create_memory_item(&memory_item(&bank, &unique("opq-i"), &subject), "tester")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("opq-req")))
        .unwrap();
    let restored = unique("opq-restored");
    let err = store
        .apply_batch(&[BatchWrite::PutOpaqueItem {
            item: opaque_item(&bank, &restored, &subject),
            actor: "tester".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    let rows: i64 = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM items WHERE id = ?1",
                [&restored],
                |r| r.get(0),
            )
            .map_err(|e| AmError::new(ErrorCode::Internal, e.to_string()))
        })
        .unwrap();
    assert_eq!(
        rows, 0,
        "opaque import created a row for the erased subject"
    );
}

#[test]
fn pg_put_opaque_item_refuses_erased_subject() {
    let _guard = crate::pg_lock::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = match PostgresStore::open(url.as_str(), local_kms()) {
        Ok(store) => store,
        Err(err) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("pg opaque shred guard test", &err)
            );
            return;
        }
    };
    let bank = unique("pg-opq-b");
    let subject = unique("pg-opq-s");
    store
        .create_memory_item(&memory_item(&bank, &unique("pg-opq-i"), &subject), "tester")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("pg-opq-req")))
        .unwrap();
    let restored = unique("pg-opq-restored");
    let err = store
        .apply_batch(&[BatchWrite::PutOpaqueItem {
            item: opaque_item(&bank, &restored, &subject),
            actor: "tester".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    let rows: i64 = store
        .with_client(|client| {
            client
                .query_one("SELECT COUNT(*) FROM items WHERE id = $1", &[&restored])
                .map_err(|e| AmError::new(ErrorCode::Internal, e.to_string()))
                .map(|row| row.get(0))
        })
        .unwrap();
    assert_eq!(
        rows, 0,
        "opaque import created a row for the erased subject"
    );
    store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}';"
        ))
        .unwrap();
}
