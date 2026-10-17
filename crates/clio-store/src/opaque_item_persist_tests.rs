// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing SQLite opaque item writes after subject erasure.
//!
//! ## Owns
//! - SQLite regression coverage for refusing erased-subject opaque writes
//!
//! ## Does not own
//! - Opaque row persistence (`opaque_item_persist`)
//! - Erasure fixtures (`erase_test_support`)
//!
//! ## Boundary
//!
//! Changes to SQLite erased-subject opaque write behavior should normally require changes here. Changes to fixture shape should require `erase_test_support`.
//!
//! Keep this module focused on the opaque write erasure contract.
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::compliance_store::ComplianceStore;
use crate::erase_test_support::{local_kms, memory_item, opaque_write, spec, unique};
use crate::migrate::IntoAmError;
use crate::model::OpaqueItemRecord;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::ErrorCode;
use rusqlite::params;
use std::sync::Arc;

#[test]
fn opaque_item_write_for_erased_subject_returns_error_without_creating_row() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("opaque-erased-bank");
    let subject = unique("opaque-erased-subject");
    let item_id = unique("opaque-erased-seeded-item");
    let opaque_item_id = unique("opaque-erased-new-item");
    store
        .create_memory_item(&memory_item(&bank, &item_id, &subject), "tester")
        .unwrap();
    let cipher = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("opaque-erased-request")))
        .unwrap();

    let result = store.apply_batch(&[BatchWrite::PutOpaqueItem {
        item: OpaqueItemRecord {
            id: opaque_item_id.clone(),
            bank_id: bank.clone(),
            subject_id: subject.clone(),
            kind: "semantic".into(),
            category: Some("tool_config".into()),
            episodic_type: None,
            epistemic_kind: "fact".into(),
            source_type: None,
            confidence: Some(0.8),
            admission_score: Some(0.0),
            source_ref: Some("turn_9".into()),
            valid_from: "t0".into(),
            valid_until: None,
            tx_from: "t0".into(),
            tx_until: None,
            created_at: "t0".into(),
            updated_at: "t0".into(),
            content_cipher: cipher,
        },
        actor: "import".into(),
    }]);
    let opaque_row_count = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM items WHERE id = ?1",
                params![opaque_item_id],
                |row| row.get::<_, i64>(0),
            )
            .am("read opaque item row count")
        })
        .unwrap();
    assert!(
        matches!(&result, Err(err) if err.code == ErrorCode::ErasedSubject),
        "opaque write result: {result:?}; new row count: {opaque_row_count}"
    );
    assert_eq!(opaque_row_count, 0, "refused opaque item has no row");
    assert_eq!(
        store.get_memory_item(&bank, &item_id).unwrap_err().code,
        ErrorCode::ErasedSubject,
        "erased item content stays unreadable"
    );
}

/// Erasure keeps a shredded `subject_keys` row, and the erased probe keys on
/// that row. Deleting the row therefore makes the subject look active again by
/// policy: the probe treats a MISSING row as NOT erased. This pins that
/// documented outcome, so a future change to the probe has to intend it.
#[test]
fn opaque_write_after_erasure_is_accepted_once_the_subject_keys_row_is_deleted() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("opaque-missing-sk-bank");
    let subject = unique("opaque-missing-sk-subject");
    let seeded_id = unique("opaque-missing-sk-seeded");
    store
        .create_memory_item(&memory_item(&bank, &seeded_id, &subject), "tester")
        .unwrap();
    let cipher = store
        .inspect_content_ciphertext(&bank, &seeded_id)
        .unwrap()
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("opaque-missing-sk-request")))
        .unwrap();
    store
        .test_execute(&format!(
            "DELETE FROM subject_keys WHERE subject_id = '{subject}'"
        ))
        .unwrap();

    let late_id = unique("opaque-missing-sk-late");
    let result = store.apply_batch(&[opaque_write(&bank, &late_id, &subject, &cipher)]);
    assert!(
        result.is_ok(),
        "a missing subject_keys row is not erased: {result:?}"
    );
    let rows = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM items WHERE id = ?1",
                params![late_id],
                |row| row.get::<_, i64>(0),
            )
            .am("late opaque row count")
        })
        .unwrap();
    assert_eq!(rows, 1, "the accepted opaque write stored a row");
}
