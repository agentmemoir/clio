// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that `append_confidence` refuses a crypto-shredded
//! subject and writes no confidence entry when it does.
//!
//! ## Owns
//! - SQLite + Postgres regression coverage for the belief append path after
//!   erasure
//!
//! ## Does not own
//! - The append transaction body and its erasure guard (`belief_tx`)
//! - The belief fixtures (`belief_store_belief_tests`) and erasure fixtures
//!   (`erase_test_support`)
//!
//! ## Boundary
//!
//! Changes to the belief append erasure contract should normally update tests
//! here. Changes to fixture shape should require the owning fixture module.
//!
//! Test-only. Never compiled into production builds.

use crate::belief_store::BeliefStore;
use crate::belief_store::belief_store_belief_tests::mk_belief;
use crate::compliance_store::ComplianceStore;
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::{local_kms, spec, unique};
use crate::migrate::IntoAmError;
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use clio_types::{ConfidenceEntry, ErrorCode, SourceType};
use std::sync::Arc;

fn append_entry() -> ConfidenceEntry {
    ConfidenceEntry::new("2026-07-14", 0.65, "turn_340", SourceType::AgentInferred).expect("entry")
}

fn sqlite_entry_count(store: &SqliteStore, belief_id: &str) -> i64 {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM belief_confidence_entries WHERE belief_id = ?1",
                rusqlite::params![belief_id],
                |row| row.get(0),
            )
            .am("count confidence entries")
        })
        .unwrap()
}

fn pg_entry_count(store: &PostgresStore, belief_id: &str) -> i64 {
    store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT COUNT(*) FROM belief_confidence_entries WHERE belief_id = $1",
                    &[&belief_id],
                )
                .am("count confidence entries")
                .map(|row| row.get::<_, i64>(0))
        })
        .unwrap()
}

#[test]
fn sqlite_append_confidence_for_erased_subject_is_refused_without_entry() {
    let kms = local_kms();
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("bel-erase-bank");
    let subject = unique("bel-erase-subject");
    let belief_id = unique("bel-erase");
    store
        .create_belief(&mk_belief(&belief_id, &bank, &subject), "agent")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("bel-erase-request")))
        .unwrap();
    let before = sqlite_entry_count(&store, &belief_id);
    assert_eq!(before, 1, "belief create writes one confidence entry");

    let err = store
        .append_confidence(&bank, &belief_id, &append_entry(), "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject, "{err}");
    assert_eq!(
        sqlite_entry_count(&store, &belief_id),
        before,
        "a refused append adds no confidence entry"
    );
}

#[test]
fn postgres_append_confidence_for_erased_subject_is_refused_without_entry() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = local_kms();
    let store = match PostgresStore::open(url.as_str(), Arc::clone(&kms)) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("belief append erasure guard", &err);
            return;
        }
    };
    let bank = unique("pg-bel-erase-bank");
    let subject = unique("pg-bel-erase-subject");
    let belief_id = unique("pg-bel-erase");
    store
        .create_belief(&mk_belief(&belief_id, &bank, &subject), "agent")
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("pg-bel-erase-request")))
        .unwrap();
    let before = pg_entry_count(&store, &belief_id);
    assert_eq!(before, 1, "belief create writes one confidence entry");

    let err = store
        .append_confidence(&bank, &belief_id, &append_entry(), "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject, "{err}");
    assert_eq!(
        pg_entry_count(&store, &belief_id),
        before,
        "a refused append adds no confidence entry"
    );
    store
        .test_execute(&format!(
            "DELETE FROM belief_confidence_entries WHERE belief_id = '{belief_id}'; \
             DELETE FROM beliefs WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}'; \
             DELETE FROM compliance_tombstones WHERE bank_id = '{bank}';"
        ))
        .unwrap();
}
