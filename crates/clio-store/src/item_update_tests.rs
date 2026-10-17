// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that `update_memory_item` refuses a crypto-shredded
//! subject and changes no stored content when it does.
//!
//! ## Owns
//! - SQLite + Postgres regression coverage for the update path after erasure
//!
//! ## Does not own
//! - The update transaction body and its erasure guard (`item_update`)
//! - The erasure fixtures (`erase_test_support`)
//!
//! ## Boundary
//!
//! Changes to the update-path erasure contract should normally update tests
//! here. Changes to fixture shape should require `erase_test_support`.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::ComplianceStore;
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::{local_kms, memory_item, spec, unique};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{ErrorCode, MemoryItem};
use std::sync::Arc;

/// A same-subject revision of `seeded` with new gist and stamp: the update
/// would rewrite content if it were allowed through.
fn revised(seeded: &MemoryItem) -> MemoryItem {
    let mut revised = seeded.clone();
    revised.gist = Some("revised after erasure".into());
    revised.updated_at = "2026-07-01T00:00:00Z".into();
    revised
}

#[test]
fn sqlite_update_for_erased_subject_is_refused_and_content_unchanged() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("upd-erase-bank");
    let subject = unique("upd-erase-subject");
    let item_id = unique("upd-erase-item");
    let seeded = memory_item(&bank, &item_id, &subject);
    store.create_memory_item(&seeded, "tester").unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("upd-erase-request")))
        .unwrap();
    let before = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .expect("seeded content ciphertext");

    let err = store
        .update_memory_item(&revised(&seeded), "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject, "{err}");
    let after = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .expect("content ciphertext survives the refused update");
    assert_eq!(before, after, "a refused update must not rewrite content");
}

#[test]
fn postgres_update_for_erased_subject_is_refused_and_content_unchanged() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = local_kms();
    let store = match PostgresStore::open(url.as_str(), Arc::clone(&kms)) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("item update erasure guard", &err);
            return;
        }
    };
    let bank = unique("pg-upd-erase-bank");
    let subject = unique("pg-upd-erase-subject");
    let item_id = unique("pg-upd-erase-item");
    let seeded = memory_item(&bank, &item_id, &subject);
    store.create_memory_item(&seeded, "tester").unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("pg-upd-erase-request")))
        .unwrap();
    let before = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .expect("seeded content ciphertext");

    let err = store
        .update_memory_item(&revised(&seeded), "tester")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject, "{err}");
    let after = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .expect("content ciphertext survives the refused update");
    assert_eq!(before, after, "a refused update must not rewrite content");
    store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}'; \
             DELETE FROM compliance_tombstones WHERE bank_id = '{bank}';"
        ))
        .unwrap();
}
