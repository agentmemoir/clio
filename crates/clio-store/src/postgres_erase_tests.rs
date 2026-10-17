// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that Postgres opaque writes refuse erased subjects.
//!
//! ## Owns
//! - Postgres regression coverage for opaque writes after subject erasure
//!
//! ## Does not own
//! - Postgres erasure SQL (`postgres_erase`)
//! - Opaque item upsert SQL (`opaque_item_persist`)
//!
//! ## Boundary
//!
//! Changes to the Postgres erased-subject opaque write contract should normally require changes here.
//! Changes to erasure fixtures should require `erase_test_support`.
//!
//! Keep this module focused on the Postgres opaque write erasure contract.
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::compliance_store::ComplianceStore;
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::{local_kms, memory_item, spec, unique};
use crate::migrate::IntoAmError;
use crate::model::OpaqueItemRecord;
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::ErrorCode;
use std::sync::Arc;

#[test]
fn opaque_item_write_for_erased_subject_returns_error_without_creating_row() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = local_kms();
    let store = match PostgresStore::open(url.as_str(), Arc::clone(&kms)) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("opaque item erasure guard", &err);
            return;
        }
    };
    let bank = unique("opaque-erased-pg-bank");
    let subject = unique("opaque-erased-pg-subject");
    let item_id = unique("opaque-erased-pg-seeded-item");
    let opaque_item_id = unique("opaque-erased-pg-new-item");
    store
        .create_memory_item(&memory_item(&bank, &item_id, &subject), "tester")
        .unwrap();
    let cipher = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("opaque-erased-pg-request")))
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
        .with_client(|client| {
            Ok(client
                .query_one(
                    "SELECT COUNT(*) FROM items WHERE id = $1",
                    &[&opaque_item_id],
                )
                .am("read opaque item row count")?
                .get::<_, i64>(0))
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
    store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}';"
        ))
        .unwrap();
}
