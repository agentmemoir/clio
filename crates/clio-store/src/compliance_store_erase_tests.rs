// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for compliance-erasure store contract test
//! scenarios (SQLite and Postgres) using the shared fixtures.
//!
//! ## Owns
//! - Erasure test scenarios: shred/purge, validation, hub referrers,
//!   cross-bank fan-out
//!
//! ## Does not own
//! - Test fixtures and shared assertions (`erase_test_support`)
//! - The erasure transaction under test (`sqlite_erase` / `postgres_erase`)
//!
//! ## Boundary
//!
//! Changes to erasure scenario coverage should normally require changes here.
//! Changes to fixture shape should normally require `erase_test_support`.
//!
//! Keep this module focused on erasure test scenarios.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::ComplianceStore;
use crate::erase_test_support::{
    assert_erased, link_hub, local_kms, memory_item, restore_legacy_format_row, seed, seed_ghost,
    seed_hub, spec, unique, validation_arms,
};
use crate::index_store::IndexStore;
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::ErrorCode;
use std::sync::Arc;

#[test]
fn sqlite_erase_shreds_and_purges() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(kms).unwrap();
    let bank = unique("sqlite-erase");
    let subject = unique("subject");
    let (item_a, parent) = seed(&store, &bank, &subject);
    let request = unique("req");

    let listed = store.list_subject_items(&bank, &subject).unwrap();
    assert_eq!(listed.len(), 2);

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert_eq!(outcome.item_ids.len(), 2);
    assert_eq!(outcome.item_hashes.len(), 2);
    assert!(outcome.vectors_purged >= 1);
    assert!(outcome.assoc_edges_purged >= 1);
    assert!(outcome.memtree_nodes_purged >= 2);
    assert_eq!(outcome.tombstones_written, 2);

    assert_erased(&store, &store, &bank, &subject, &request, &parent, &item_a);
}

/// An erase after indexing clears the lexical derived copy. The
/// `items_fts` row the index wrote is gone once the subject is erased, so the
/// erased content keeps no readable (or digest) copy in the index.
#[test]
fn sqlite_erase_clears_the_lexical_derived_row() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-erase-lex");
    let subject = unique("subject");
    let item = unique("item-lex");
    store
        .create_memory_item(&memory_item(&bank, &item, &subject), "tester")
        .unwrap();
    IndexStore::put_lexical_doc(&store, &bank, &item, "kryptonite zephyr").unwrap();

    let fts_rows = |store: &SqliteStore, item: &str, bank: &str| -> i64 {
        store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM items_fts WHERE item_id = ?1 AND bank_id = ?2",
                    rusqlite::params![item, bank],
                    |row| row.get(0),
                )
                .am("count items_fts rows")
            })
            .unwrap()
    };
    assert_eq!(fts_rows(&store, &item, &bank), 1, "row exists before erase");

    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();

    assert_eq!(
        fts_rows(&store, &item, &bank),
        0,
        "the erase cleared the lexical derived copy"
    );
}

#[test]
fn sqlite_list_tombstones_orders_nulls_last_then_hash() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-tomb-order");
    let subject = unique("subject");
    let (_item_a, _parent) = seed(&store, &bank, &subject);
    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();

    // A subject with a key but no items writes a subject-level (null hash) row.
    let key_only = unique("key-only");
    store
        .upsert_subject_key_ref(&key_only, "local:key-only:v1", "active")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &key_only, &unique("req")))
        .unwrap();

    let toms = store.list_tombstones(&bank).unwrap();
    let hashes: Vec<Option<&str>> = toms.iter().map(|t| t.item_id_hash.as_deref()).collect();
    let first_null = hashes
        .iter()
        .position(|h| h.is_none())
        .expect("key-only tombstone has no item hash");
    let non_null: Vec<&str> = hashes[..first_null].iter().filter_map(|h| *h).collect();
    assert_eq!(non_null.len(), 2);
    assert!(hashes[first_null..].iter().all(Option::is_none));
    let mut sorted = non_null.clone();
    sorted.sort_unstable();
    assert_eq!(non_null, sorted, "non-null item hashes ascending");
}

#[test]
fn sqlite_erase_unknown_subject_is_not_found() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-erase-miss");
    let subject = unique("missing-subject");
    let request = unique("req");
    let err = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let toms = store.read_tombstones(&bank, &request).unwrap();
    assert!(toms.is_empty(), "{toms:?}");
    let items = store.list_subject_items(&bank, &subject).unwrap();
    assert!(items.is_empty(), "{items:?}");
}

#[test]
fn sqlite_erase_legacy_format_row_read_fails_closed() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("sqlite-erase-env");
    let subject = unique("subject");
    let item = unique("env-item");
    restore_legacy_format_row(&store, kms.as_ref(), &item, &bank, &subject);
    let request = unique("req");
    store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert_eq!(
        store.get_memory_item(&bank, &item).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

#[test]
fn postgres_erase_shreds_and_purges() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-erase");
    let subject = unique("subject");
    let (item_a, parent) = seed(&store, &bank, &subject);
    let request = unique("req");

    let listed = store.list_subject_items(&bank, &subject).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(!store.subject_dek_destroyed(&subject).unwrap());

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert_eq!(outcome.item_ids.len(), 2);
    assert!(outcome.vectors_purged >= 1);
    assert!(outcome.assoc_edges_purged >= 1);
    assert!(outcome.memtree_nodes_purged >= 2);

    assert_erased(&store, &store, &bank, &subject, &request, &parent, &item_a);
    assert_eq!(
        store.get_memory_item(&bank, &item_a).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

/// PostgreSQL half: after the erase, no item row carries a
/// non-null `content_tsv` for the erased item. The row itself may be gone; a
/// surviving row must at least have lost its derived column.
#[test]
fn postgres_erase_clears_the_lexical_derived_column() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-erase-lex");
    let subject = unique("subject");
    let item = unique("item-lex");
    store
        .create_memory_item(&memory_item(&bank, &item, &subject), "tester")
        .unwrap();
    IndexStore::put_lexical_doc(&store, &bank, &item, "kryptonite zephyr").unwrap();

    let tsv_present = |store: &PostgresStore, item: &str, bank: &str| -> i64 {
        store
            .with_client(|client| {
                client
                    .query_one(
                        "SELECT COUNT(*) AS n FROM items
                         WHERE id = $1 AND bank_id = $2 AND content_tsv IS NOT NULL",
                        &[&item, &bank],
                    )
                    .am("count rows with content_tsv")
                    .map(|row| row.get::<_, i64>(0))
            })
            .unwrap()
    };
    assert_eq!(
        tsv_present(&store, &item, &bank),
        1,
        "derived column present before the erase"
    );

    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();

    assert_eq!(
        tsv_present(&store, &item, &bank),
        0,
        "the erase cleared the lexical derived copy"
    );
}

#[test]
fn sqlite_erase_validation_ghost_and_probe_arms() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-erase-edges");
    let subject = unique("subject");
    let request = unique("req");
    validation_arms(&store, &bank, &subject, &request);

    let ghost_subject = unique("ghost-subject");
    seed_ghost(&store, &bank, &ghost_subject);
    let ghost_request = unique("req");
    let outcome = store
        .erase_subject(&spec(&bank, &ghost_subject, &ghost_request))
        .unwrap();
    assert_eq!(outcome.memtree_nodes_purged, 1);
}

#[test]
fn postgres_erase_validation_ghost_and_probe_arms() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-erase-edges");
    let subject = unique("subject");
    let request = unique("req");
    validation_arms(&store, &bank, &subject, &request);

    // Unknown subject: no invalid tombstone.
    let missing = unique("missing");
    let miss_req = unique("req");
    let err = store
        .erase_subject(&spec(&bank, &missing, &miss_req))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let missing_tombs = store.read_tombstones(&bank, &miss_req).unwrap();
    assert!(missing_tombs.is_empty(), "{missing_tombs:?}");

    let ghost_subject = unique("ghost-subject");
    seed_ghost(&store, &bank, &ghost_subject);
    let ghost_request = unique("req");
    let outcome = store
        .erase_subject(&spec(&bank, &ghost_subject, &ghost_request))
        .unwrap();
    assert_eq!(outcome.memtree_nodes_purged, 1);

    // A subject with a key but no items still shreds and writes one
    // subject-level tombstone with no item hash.
    let key_only = unique("key-only");
    store
        .upsert_subject_key_ref(&key_only, "local:key-only:v1", "active")
        .unwrap();
    let ko_request = unique("req");
    let outcome = store
        .erase_subject(&spec(&bank, &key_only, &ko_request))
        .unwrap();
    assert!(outcome.item_ids.is_empty(), "{:?}", outcome.item_ids);
    assert_eq!(outcome.tombstones_written, 1);
    let toms = store.read_tombstones(&bank, &ko_request).unwrap();
    assert_eq!(toms.len(), 1);
    assert!(toms[0].item_id_hash.is_none());
    assert!(store.subject_dek_destroyed(&key_only).unwrap());
}

#[test]
fn sqlite_erase_purges_referencing_derived_hubs() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-hub");
    let subject = unique("subject");
    let item_a = unique("item");
    store
        .create_memory_item(&memory_item(&bank, &item_a, &subject), "tester")
        .unwrap();
    let hub_subject = unique("hub-subject");
    let hub = seed_hub(&store, &bank, &hub_subject);
    link_hub(&store, &bank, &hub, &item_a);
    assert!(store.get_snapshot(&bank, &hub).unwrap().is_some());

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.derived_items_purged, 1);
    assert!(store.get_snapshot(&bank, &hub).is_err());
    // The hub's own subject DEK is untouched.
    assert!(!store.subject_dek_destroyed(&hub_subject).unwrap());
}

#[test]
fn postgres_erase_purges_referencing_derived_hubs() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let bank = unique("pg-hub");
    let subject = unique("subject");
    let item_a = unique("item");
    store
        .create_memory_item(&memory_item(&bank, &item_a, &subject), "tester")
        .unwrap();
    let hub_subject = unique("hub-subject");
    let hub = seed_hub(&store, &bank, &hub_subject);
    link_hub(&store, &bank, &hub, &item_a);
    assert!(store.get_snapshot(&bank, &hub).unwrap().is_some());

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.derived_items_purged, 1);
    assert!(store.get_snapshot(&bank, &hub).is_err());
    assert!(!store.subject_dek_destroyed(&hub_subject).unwrap());
}

#[test]
fn sqlite_erase_fans_out_to_other_banks_before_dek_shred() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let home = unique("home-bank");
    let request_bank = unique("request-bank");
    let subject = unique("subject");
    let (item_a, parent) = seed(&store, &home, &subject);

    // Erase is requested from a bank where the subject has zero items.
    let outcome = store
        .erase_subject(&spec(&request_bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.item_ids.len(), 2);
    assert!(outcome.vectors_purged >= 1);
    assert!(outcome.memtree_nodes_purged >= 2);
    assert!(outcome.assoc_edges_purged >= 1);
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        store.get_memory_item(&home, &item_a).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    // The home bank's derived structures were purged: ancestor dirty, gist NULL.
    let node = store.get_memtree_node(&home, &parent).unwrap().expect("p");
    assert!(node.dirty);
    assert!(node.summary_gist.is_none());
    // Tombstone ledger stays request-scoped to the requesting bank.
    assert!(store.read_tombstones(&request_bank, "req").is_ok());
}

#[test]
fn postgres_erase_fans_out_to_other_banks_before_dek_shred() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(url.as_str(), local_kms()).unwrap();
    let home = unique("pg-home-bank");
    let request_bank = unique("pg-request-bank");
    let subject = unique("subject");
    let (item_a, parent) = seed(&store, &home, &subject);

    let outcome = store
        .erase_subject(&spec(&request_bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(outcome.item_ids.len(), 2);
    assert!(outcome.vectors_purged >= 1);
    assert!(outcome.memtree_nodes_purged >= 2);
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        store.get_memory_item(&home, &item_a).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    let node = store.get_memtree_node(&home, &parent).unwrap().expect("p");
    assert!(node.dirty);
    assert!(node.summary_gist.is_none());
}

#[path = "compliance_store_erase_tests_persona_tests.rs"]
mod persona_context;
