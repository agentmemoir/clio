// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the peer_actions slot's real column set on
//! both backends: the DDL declares a feature schema, not a placeholder, so a
//! column rename, drop, or reorder must update this pin rather than slip past
//! the slot tests.
//!
//! ## Owns
//! - The peer_actions column list and the SQLite/Postgres assertions over it
//!
//! ## Does not own
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - The other slot column pins (`migrate_compliance_slots_tests`), reused here
//! - The begin/advance behavior of the store (`peer_actions_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the peer_actions column set or its order changes.
//!
//! Test-only. Never compiled into production builds.

use super::{Scratch, ScratchDatabase, postgres_columns, sqlite_columns, url_for_database};
use crate::PostgresStore;
use crate::erase_test_support::local_kms;
use crate::sqlite::SqliteStore;

/// The shared URL, or `None` after the fail-closed skip line: an unreachable
/// Postgres fails this suite unless the operator explicitly opted out.
fn pg_url_or_skip() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres slot column tests", &err);
        return None;
    }
    Some(url)
}

/// The `peer_actions` column set, in DDL order.
const PEER_ACTION_COLUMNS: [&str; 7] = [
    "request_id",
    "peer_id",
    "action_kind",
    "action_id",
    "state",
    "disposition",
    "proof_ref",
];

/// The fresh SQLite peer_actions slot holds the guarded action columns, in DDL
/// order.
#[test]
fn the_peer_actions_slot_holds_the_action_columns() {
    let scratch = Scratch::new();
    let store = SqliteStore::open(&scratch.0.join("store.db"), local_kms())
        .expect("open a fresh sqlite store");
    assert_eq!(
        sqlite_columns(&store, "peer_actions"),
        PEER_ACTION_COLUMNS.map(str::to_owned).to_vec(),
        "peer_actions column set changed"
    );
}

/// The fresh Postgres peer_actions slot holds the same column set.
#[test]
fn postgres_peer_actions_slot_holds_the_action_columns() {
    let _shared = crate::lock_pg_test();
    let Some(url) = pg_url_or_skip() else {
        return;
    };
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, "peeractions");
    drop(admin);
    let store = PostgresStore::open(&url_for_database(&db.name), local_kms())
        .expect("open a fresh postgres store");
    assert_eq!(
        postgres_columns(&store, "peer_actions"),
        PEER_ACTION_COLUMNS.map(str::to_owned).to_vec(),
        "peer_actions column set changed"
    );
}
