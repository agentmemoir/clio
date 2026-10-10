// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite row mapping helpers shared by the
//! sync-journal persistence (`sqlite_sync.rs`): row readers for journal,
//! conflict, dead-letter, and feed rows.
//!
//! ## Owns
//! - SQLite sync row-mapping functions
//!
//! ## Does not own
//! - SQL bodies, column lists, or trait impls (`sqlite_sync.rs` / `sync_store.rs`)
//! - Postgres sync persistence (`postgres_sync`)
//!
//! ## Boundary
//!
//! Changes to the sync row shapes should normally require changes here and in
//! `sync_store.rs` together. Changes to sync semantics should not.
//!
//! Keep this module focused on SQLite sync row mapping.

use crate::sync_store::{
    DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncFeedRow, SyncJournalRow,
};
use rusqlite::Row;

pub(crate) fn read_journal_row(row: &Row<'_>) -> rusqlite::Result<SyncJournalRow> {
    Ok(SyncJournalRow {
        seq: row.get(0)?,
        sync_mutation_id: row.get(1)?,
        bank_id: row.get(2)?,
        entity_kind: row.get(3)?,
        entity_id: row.get(4)?,
        op: row.get(5)?,
        transaction_time: row.get(6)?,
        device_id: row.get(7)?,
        content_hash: row.get(8)?,
        content_sealed: row.get(9)?,
        client_encrypted: row.get::<_, i64>(10)? != 0,
        admission_receipt: row.get(11)?,
        created_at: row.get(12)?,
    })
}

pub(crate) fn read_conflict_row(row: &Row<'_>) -> rusqlite::Result<SyncConflictRow> {
    Ok(SyncConflictRow {
        bank_id: row.get(0)?,
        entity_kind: row.get(1)?,
        entity_id: row.get(2)?,
        winner_mutation_id: row.get(3)?,
        loser_mutation_id: row.get(4)?,
        winner_transaction_time: row.get(5)?,
        loser_transaction_time: row.get(6)?,
        winner_device_id: row.get(7)?,
        loser_device_id: row.get(8)?,
        created_at: row.get(9)?,
    })
}

pub(crate) fn read_dlq_row(row: &Row<'_>) -> rusqlite::Result<SyncDeadLetterRow> {
    Ok(SyncDeadLetterRow {
        sync_mutation_id: row.get(0)?,
        bank_id: row.get(1)?,
        reason_code: row.get(2)?,
        detail: row.get(3)?,
        envelope_json: row.get(4)?,
        status: DeadLetterStatus::parse(&row.get::<_, String>(5)?)
            .unwrap_or(DeadLetterStatus::Open),
        first_seen: row.get(6)?,
        last_error: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

/// Classify an item feed row into an operation hint from its stamp columns.
pub(crate) fn feed_item_row(
    id: &str,
    _tx_from: &str,
    tx_until: Option<&str>,
    discarded_at: Option<&str>,
    stamp: &str,
) -> SyncFeedRow {
    let op = if discarded_at.is_some() {
        "discard"
    } else if tx_until.is_some() {
        "invalidate"
    } else {
        "upsert"
    };
    SyncFeedRow {
        entity_kind: "item".into(),
        entity_id: id.to_owned(),
        op: op.to_owned(),
        transaction_time: stamp.to_owned(),
    }
}
