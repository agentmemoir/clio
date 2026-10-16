// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `BankScopeStore` dispatch tests: reading
//! the live erasure bank set through a `dyn` trait object on each backend.
//!
//! ## Owns
//! - The bank-set scenario: items across banks plus the requesting bank
//!
//! ## Does not own
//! - Test fixtures or the erasure transaction (`erase_test_support`, `*_erase`)
//!
//! ## Boundary
//!
//! Changes to the bank-set scenario should normally require changes here.
//! Changes to the erasure transaction should not.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::BankScopeStore;
use crate::dek::{DekProvider, LocalDevKms};
use crate::erase_test_support::{memory_item, unique};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use std::sync::Arc;

fn seed_subject_items(store: &dyn Store, banks: &[String], subject: &str) {
    for bank in banks {
        let id = unique("bank-scope-item");
        store
            .create_memory_item(&memory_item(bank, &id, subject), "tester")
            .unwrap();
    }
}

/// The bank set is every bank holding the subject's items plus the requesting
/// bank, sorted, read through the trait object.
fn run_bank_scope<T: Store + BankScopeStore>(store: &T) {
    let subject = unique("bank-scope-subject");
    let bank_a = unique("bank-scope-a");
    let bank_b = unique("bank-scope-b");
    let requesting = unique("bank-scope-req");
    seed_subject_items(store, &[bank_a.clone(), bank_b.clone()], &subject);

    let scoped: &dyn BankScopeStore = store;
    let mut expected = vec![bank_a, bank_b, requesting.clone()];
    expected.sort();
    assert_eq!(
        scoped.erase_bank_set(&requesting, &subject).unwrap(),
        expected
    );
}

#[test]
fn sqlite_bank_scope_lists_every_affected_bank() {
    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap();
    run_bank_scope(&store);
}

#[test]
fn postgres_bank_scope_lists_every_affected_bank() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(&url, kms).unwrap_or_else(|e| panic!("postgres required: {e}"));
    run_bank_scope(&store);
}
