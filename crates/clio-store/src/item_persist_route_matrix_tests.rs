// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the erased-subject route matrix: one loop
//! that proves every non-key-creating write route refuses a crypto-shredded
//! subject with `ErasedSubject`, on both backends.
//!
//! ## Owns
//! - The table of write routes and the shared erased-subject assertion
//!
//! ## Does not own
//! - The per-route erasure guards (`item_update`, `belief_tx`,
//!   `opaque_item_persist`)
//! - The erasure fixtures (`erase_test_support`)
//!
//! ## Boundary
//!
//! A new non-key-creating write route belongs in the route table here as one
//! line. Changes to the shared erased-subject error require `item_persist`.
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::BatchStore;
use crate::belief_store::BeliefStore;
use crate::belief_store::belief_store_belief_tests::mk_belief;
use crate::compliance_store::ComplianceStore;
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::{local_kms, memory_item, opaque_write, spec, unique};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{AmError, ConfidenceEntry, ErrorCode, MemoryItem, SourceType};
use std::sync::Arc;

/// Same-subject revision of `seeded` that would rewrite content if allowed.
fn revised(seeded: &MemoryItem) -> MemoryItem {
    let mut revised = seeded.clone();
    revised.gist = Some("revised after erasure".into());
    revised.updated_at = "2026-07-01T00:00:00Z".into();
    revised
}

fn append_entry() -> ConfidenceEntry {
    ConfidenceEntry::new("2026-07-14", 0.65, "turn_340", SourceType::AgentInferred).expect("entry")
}

#[test]
fn sqlite_every_non_key_creating_write_route_refuses_an_erased_subject() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("route-matrix-bank");
    let subject = unique("route-matrix-subject");
    let item_id = unique("route-matrix-item");
    let belief_id = unique("route-matrix-belief");
    store
        .create_memory_item(&memory_item(&bank, &item_id, &subject), "tester")
        .unwrap();
    store
        .create_belief(&mk_belief(&belief_id, &bank, &subject), "agent")
        .unwrap();
    let cipher = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("route-matrix-request")))
        .unwrap();

    let seeded = memory_item(&bank, &item_id, &subject);
    let opaque_id = unique("route-matrix-opaque");
    let entry = append_entry();
    // One line per route, so adding a route is one entry.
    let routes: Vec<(&str, Box<dyn Fn() -> Result<(), AmError>>)> = vec![
        (
            "opaque_write",
            Box::new(|| store.apply_batch(&[opaque_write(&bank, &opaque_id, &subject, &cipher)])),
        ),
        (
            "item_update",
            Box::new(|| store.update_memory_item(&revised(&seeded), "tester")),
        ),
        (
            "confidence_append",
            Box::new(|| store.append_confidence(&bank, &belief_id, &entry, "agent")),
        ),
    ];
    for (route, run) in routes {
        let err = run().unwrap_err();
        assert_eq!(err.code, ErrorCode::ErasedSubject, "route {route}: {err}");
    }
}

#[test]
fn postgres_every_non_key_creating_write_route_refuses_an_erased_subject() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = local_kms();
    let store = match PostgresStore::open(url.as_str(), Arc::clone(&kms)) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("erased-subject route matrix", &err);
            return;
        }
    };
    let bank = unique("pg-route-matrix-bank");
    let subject = unique("pg-route-matrix-subject");
    let item_id = unique("pg-route-matrix-item");
    let belief_id = unique("pg-route-matrix-belief");
    store
        .create_memory_item(&memory_item(&bank, &item_id, &subject), "tester")
        .unwrap();
    store
        .create_belief(&mk_belief(&belief_id, &bank, &subject), "agent")
        .unwrap();
    let cipher = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("pg-route-matrix-request")))
        .unwrap();

    let seeded = memory_item(&bank, &item_id, &subject);
    let opaque_id = unique("pg-route-matrix-opaque");
    let entry = append_entry();
    // One line per route, so adding a route is one entry.
    let routes: Vec<(&str, Box<dyn Fn() -> Result<(), AmError>>)> = vec![
        (
            "opaque_write",
            Box::new(|| store.apply_batch(&[opaque_write(&bank, &opaque_id, &subject, &cipher)])),
        ),
        (
            "item_update",
            Box::new(|| store.update_memory_item(&revised(&seeded), "tester")),
        ),
        (
            "confidence_append",
            Box::new(|| store.append_confidence(&bank, &belief_id, &entry, "agent")),
        ),
    ];
    for (route, run) in routes {
        let err = run().unwrap_err();
        assert_eq!(err.code, ErrorCode::ErasedSubject, "route {route}: {err}");
    }

    store
        .test_execute(&format!(
            "DELETE FROM belief_confidence_entries WHERE belief_id = '{belief_id}'; \
             DELETE FROM beliefs WHERE bank_id = '{bank}'; \
             DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}'; \
             DELETE FROM compliance_tombstones WHERE bank_id = '{bank}';"
        ))
        .unwrap();
}
