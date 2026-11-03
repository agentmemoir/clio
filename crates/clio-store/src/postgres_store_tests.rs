// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres store boundary around a sealed
//! MemTree summary gist: sealing before the write, and opening after a read.
//!
//! ## Owns
//! - Round-trip of a gist through `upsert_memtree_node`, `get_memtree_node`
//!   and `query_memtree_nodes`
//! - The at-rest form of the column, which is an envelope and not the plaintext
//! - The tolerant gist read after a bank's DEK is destroyed
//!
//! ## Does not own
//! - The envelope shape and the seal/open pair (see `memtree_seal`)
//! - MemTree SQL and row mapping (see `postgres_memtree`)
//!
//! ## Boundary
//!
//! Keep this module on the store boundary, not on the ciphertext itself.

use crate::dek::{CiphertextEnvelope, DekProvider, LocalDevKms};
use crate::memtree_model::{MemTreeNodeRecord, MemTreeQuery};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::store::Store;
use std::sync::Arc;

const GIST: &str = "we discussed the move to Lisbon and the visa deadline";

fn node(bank: &str, id: &str) -> MemTreeNodeRecord {
    MemTreeNodeRecord {
        id: id.into(),
        bank_id: bank.into(),
        parent_id: None,
        item_id: Some("item-1".into()),
        depth: 0,
        interval_start: "100".into(),
        interval_end: "100".into(),
        child_ids: vec![],
        summary_gist: Some(GIST.into()),
        dirty: true,
        generation: 1,
        refreshing: false,
        task_id: None,
        last_error: None,
        created_at: "100".into(),
        updated_at: "100".into(),
    }
}

/// Open the shared test database with the given key store.
fn open_store(kms: Arc<dyn DekProvider>) -> PostgresStore {
    let url = crate::db_url::postgres_url_from_env();
    PostgresStore::open(&url, kms).expect("the test database opens")
}

/// A bank id no other test uses, so rows from a parallel run never mix in.
fn unique_bank() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("pg-gist-{nanos}")
}

/// Read the raw `summary_gist` column, without the store's open step.
fn stored_gist(store: &PostgresStore, bank: &str, id: &str) -> Option<String> {
    store
        .with_client(|client| {
            let row = client
                .query_opt(
                    "SELECT summary_gist FROM memtree_nodes WHERE id = $1 AND bank_id = $2",
                    &[&id, &bank],
                )
                .am("raw memtree gist read")?;
            Ok(row.and_then(|r| r.get::<_, Option<String>>(0)))
        })
        .expect("the raw read succeeds")
}

/// A node id no other test uses. `memtree_nodes.id` is globally unique, and the
/// upsert's conflict path is bank-guarded, so a shared id would silently drop a
/// write meant for a different bank.
fn unique_node(bank: &str) -> String {
    format!("{bank}-n1")
}

#[test]
fn a_sealed_gist_round_trips_through_get_and_query() {
    let _guard = crate::lock_pg_test();
    let store = open_store(Arc::new(LocalDevKms::new()));
    let bank = unique_bank();
    let id = unique_node(&bank);
    store
        .upsert_memtree_node(&node(&bank, &id))
        .expect("upsert");

    let got = store
        .get_memtree_node(&bank, &id)
        .expect("get succeeds")
        .expect("the row is there");
    assert_eq!(got.summary_gist.as_deref(), Some(GIST));

    let rows = store
        .query_memtree_nodes(&MemTreeQuery {
            bank_id: bank.clone(),
            limit: Some(10),
            ..Default::default()
        })
        .expect("query succeeds");
    assert_eq!(rows.len(), 1, "one row in this bank: {rows:?}");
    assert_eq!(rows[0].summary_gist.as_deref(), Some(GIST));
}

#[test]
fn the_stored_gist_is_an_envelope_and_not_the_plaintext() {
    let _guard = crate::lock_pg_test();
    let store = open_store(Arc::new(LocalDevKms::new()));
    let bank = unique_bank();
    let id = unique_node(&bank);
    store
        .upsert_memtree_node(&node(&bank, &id))
        .expect("upsert");

    let raw = stored_gist(&store, &bank, &id).expect("a gist is stored");
    assert!(
        !raw.contains(GIST),
        "the plaintext must not reach the column: {raw}"
    );
    let envelope =
        CiphertextEnvelope::from_storage_string(&raw).expect("the column holds an envelope");
    assert_eq!(envelope.subject_id, bank);
    assert_ne!(envelope.ciphertext_b64, "", "the envelope carries a body");
}

#[test]
fn a_destroyed_bank_dek_reads_the_gist_as_absent() {
    let _guard = crate::lock_pg_test();
    let kms = Arc::new(LocalDevKms::new());
    let store = open_store(kms.clone());
    let bank = unique_bank();
    let id = unique_node(&bank);
    store
        .upsert_memtree_node(&node(&bank, &id))
        .expect("upsert");
    kms.destroy_dek(&bank).expect("the bank key is destroyed");

    let got = store
        .get_memtree_node(&bank, &id)
        .expect("a shredded bank still reads")
        .expect("the row itself is still there");
    assert_eq!(
        got.summary_gist, None,
        "an unreadable gist reads as absent rather than raising an error"
    );
    assert_eq!(got.id, id);
    assert_eq!(got.item_id.as_deref(), Some("item-1"));
}
