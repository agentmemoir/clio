// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the SQLite sync-state SQL bodies' row limits: an
//! oversized `usize` must reach SQLite as a bounded positive `LIMIT`, never as
//! the negative value SQLite reads as "no limit".
//!
//! ## Owns
//! - Coverage for `limit_i64` and for a journal read asked for `usize::MAX`
//!   rows
//!
//! ## Does not own
//! - The journal append transaction or conflict policy (`sqlite_sync`,
//!   `sync_records`)
//! - Postgres sync state SQL (`postgres_sync_state`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::limit_i64;
use crate::sqlite::sqlite_history_tests::store;
use crate::sync_store::{SyncJournalRow, SyncStore};

fn row(mid: &str) -> SyncJournalRow {
    SyncJournalRow {
        seq: 0,
        sync_mutation_id: mid.into(),
        bank_id: "b1".into(),
        entity_kind: "item".into(),
        entity_id: format!("i-{mid}"),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content_hash: format!("h-{mid}"),
        content_sealed: None,
        client_encrypted: false,
        admission_receipt: None,
        created_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn journal_list_since_bounds_an_oversized_limit_to_the_available_rows() {
    // A wrapped negative `LIMIT` also returns every available row, so the
    // saturated value is what pins the conversion itself.
    assert_eq!(limit_i64(usize::MAX), i64::MAX);
    let store = store();
    for n in 0..3 {
        store
            .journal_append(&row(&format!("m-{n}")))
            .expect("append");
    }
    let rows = store
        .journal_list_since("b1", 0, usize::MAX)
        .expect("journal list");
    let ids = rows
        .iter()
        .map(|r| r.sync_mutation_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["m-0", "m-1", "m-2"]);
}
