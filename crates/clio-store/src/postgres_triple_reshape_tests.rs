// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres refusal when a `triples` table
//! still carries the legacy plaintext `object` column.
//!
//! ## Owns
//! - Postgres parity for "a legacy-shaped store refuses to open and keeps its
//!   rows"
//!
//! ## Does not own
//! - Seal-at-rest and erasure parity (`pg_triple_seal_erase_tests.rs`)
//! - SQLite behaviour (see `triple_seal_erase_tests`, `triple_reshape_tests`)
//! - Postgres triple query semantics (`pg_triple_tests.rs`)
//!
//! ## Boundary
//!
//! Split out of `pg_triple_seal_erase_tests.rs` to keep that file inside the
//! 450-line limit. Changes to the legacy-shape refusal belong here; changes to
//! what erasure must leave behind belong in `pg_triple_seal_erase_tests.rs`.
//!
//! Keep this module focused on the legacy-shape open guard.

use crate::postgres_triple::postgres_triple_seal_erase_tests::{
    SECRET, carrier, edge, open_pg, unique_ids, unique_subject, url,
};
use crate::dek::{DekProvider, LocalDevKms};
use crate::pg_lock::lock_pg_test;
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::ErrorCode;
use std::sync::Arc;

fn pg_column_present(pg: &PostgresStore, column: &str) -> bool {
    pg.with_client(|client| {
        Ok(client
            .query_one(
                "SELECT EXISTS (
                     SELECT 1 FROM information_schema.columns
                     WHERE table_schema = current_schema() AND table_name = 'triples'
                       AND column_name = $1
                 )",
                &[&column],
            )
            .expect("column probe")
            .get(0))
    })
    .expect("column probe")
}

#[test]
fn pg_a_legacy_shaped_triples_table_refuses_to_open_and_keeps_its_rows() {
    let _guard = lock_pg_test();
    if open_pg().is_none() {
        return;
    }
    let scoped = ScopedSchema::new(&url());
    let bank = unique("pg-legacy-refuse");
    let subject = unique_subject();
    let ids = unique_ids();
    // The same KMS across both opens, so the carrier item's DEK exists either
    // way.
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = PostgresStore::open(scoped.url(), Arc::clone(&kms)).expect("open");
    // A current store reads through the sealed column and has no plaintext
    // content column; `subject`/`predicate` stay plaintext (structure).
    assert!(
        !pg_column_present(&store, "object"),
        "plaintext content column absent"
    );
    assert!(
        pg_column_present(&store, "object_ciphertext"),
        "sealed column present"
    );
    store
        .commit_triple_add(
            &carrier(&bank, &subject, &ids),
            &edge(&bank, &ids),
            true,
            "actor",
        )
        .expect("triple add");
    // Rebuild the pre-removal shape behind the store's back: a plaintext
    // `object` and no sealed column. Unqualified names resolve through the
    // scoped search_path to this test's private schema, never `public`.
    store
        .test_execute(&format!(
            "ALTER TABLE triples ADD COLUMN object text;
             UPDATE triples SET object = '{SECRET}' WHERE bank_id = '{bank}';
             ALTER TABLE triples DROP COLUMN object_ciphertext;"
        ))
        .expect("rebuild the legacy shape");
    drop(store);

    let Err(err) = PostgresStore::open(scoped.url(), Arc::clone(&kms)) else {
        panic!("legacy shape must refuse to open");
    };
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.to_string().contains("legacy plaintext"), "{err}");
    assert!(err.to_string().contains("no longer"), "{err}");

    // Reconnect raw to prove the refusal wrote nothing and the row survived:
    // the migrate batch never ran, so the sealed column is still gone and the
    // legacy value was neither promoted nor deleted. The scoped URL keeps
    // `current_schema()` on the private schema, so the guard and every probe
    // below observe only this test's rows.
    let mut client = crate::pg_tls::connect(scoped.url()).expect("raw reconnect");
    let sealed_present: bool = client
        .query_one(
            "SELECT EXISTS (
                 SELECT 1 FROM information_schema.columns
                 WHERE table_schema = current_schema() AND table_name = 'triples'
                   AND column_name = 'object_ciphertext'
             )",
            &[],
        )
        .expect("sealed column probe")
        .get(0);
    assert!(
        !sealed_present,
        "the migrate batch must not have run on a refused open"
    );
    let object_value: Option<String> = client
        .query_opt(
            "SELECT object FROM triples WHERE bank_id = $1 AND id = $2",
            &[&bank, &ids.edge],
        )
        .expect("legacy row read")
        .map(|r| r.get(0));
    assert_eq!(
        object_value.as_deref(),
        Some(SECRET),
        "the original row must survive unchanged"
    );
    // No restore: dropping the private schema removes the legacy-shaped table.
}
