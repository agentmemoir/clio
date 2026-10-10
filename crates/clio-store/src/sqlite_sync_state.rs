// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite sync-protocol *state* SQL bodies
//! behind the `SyncStore` impl: the per-bank mutation
//! journal reads and guards, the opaque cursor tokens, the LWW conflict
//! journal, and the dead-letter quarantine.
//!
//! ## Owns
//! - SQLite SQL bodies for journal/cursor/conflict/dead-letter sync state
//!
//! ## Does not own
//! - The `SyncStore for SqliteStore` impl block (`sqlite_sync`), which
//!   delegates to these bodies
//! - The journal *append* transaction and the change-feed SQL (`sync_records`)
//! - Task/failure history sync persistence (`sqlite_sync_history`)
//! - Postgres sync state SQL (`postgres_sync_state`)
//!
//! ## Boundary
//!
//! Changes to SQLite sync state SQL should normally require changes here.
//! Changes to apply or conflict policy should not.
//!
//! Keep this module focused on SQLite sync state persistence.

use crate::migrate::{IntoAmError, now_iso8601};
use crate::sqlite::SqliteStore;
use crate::sqlite_sync_rows::{read_conflict_row, read_dlq_row, read_journal_row};
use crate::sync_store::{CONFLICT_COLS, DLQ_COLS, JOURNAL_COLS};
use crate::sync_store::{DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncJournalRow};
use clio_types::AmError;
use rusqlite::{OptionalExtension, params};

/// SQLite body of `SyncStore::journal_get`: one journal row by mutation id.
pub(crate) fn journal_get_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    sync_mutation_id: &str,
) -> Result<Option<SyncJournalRow>, AmError> {
    store.with_conn(|conn| {
        conn.query_row(
            &format!(
                "SELECT {JOURNAL_COLS} FROM sync_mutations \
                      WHERE bank_id = ?1 AND sync_mutation_id = ?2"
            ),
            params![bank_id, sync_mutation_id],
            read_journal_row,
        )
        .optional()
        .am("sync journal get")
    })
}

/// SQLite body of `SyncStore::journal_list_since`: journal rows past `seq`.
pub(crate) fn journal_list_since_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since_seq: i64,
    limit: usize,
) -> Result<Vec<SyncJournalRow>, AmError> {
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {JOURNAL_COLS} FROM sync_mutations \
                 WHERE bank_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3"
            ))
            .am("sync journal list prepare")?;
        let mapped = stmt
            .query_map(params![bank_id, since_seq, limit as i64], read_journal_row)
            .am("sync journal list query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync journal list map")?;
        Ok(mapped)
    })
}

/// SQLite body of `SyncStore::journal_max_seq`: the per-bank high-water seq.
pub(crate) fn journal_max_seq_sqlite(store: &SqliteStore, bank_id: &str) -> Result<i64, AmError> {
    store.with_conn(|conn| {
        conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM sync_mutations WHERE bank_id = ?1",
            params![bank_id],
            |r| r.get(0),
        )
        .am("sync journal max seq")
    })
}

/// SQLite body of `SyncStore::journal_apply_guard_hit`: the idempotency probe.
pub(crate) fn journal_apply_guard_hit_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    entity_kind: &str,
    entity_id: &str,
    op: &str,
    content_hash: &str,
) -> Result<bool, AmError> {
    store.with_conn(|conn| {
        conn.query_row(
            "SELECT 1 FROM sync_mutations WHERE bank_id = ?1 AND entity_kind = ?2 \
             AND entity_id = ?3 AND op = ?4 AND content_hash = ?5 LIMIT 1",
            params![bank_id, entity_kind, entity_id, op, content_hash],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map(|hit| hit.is_some())
        .am("sync journal apply guard")
    })
}

/// SQLite body of `SyncStore::cursor_get`: the peer's opaque cursor token.
pub(crate) fn cursor_get_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    peer_label: &str,
) -> Result<Option<String>, AmError> {
    store.with_conn(|conn| {
        conn.query_row(
            "SELECT cursor_token FROM sync_cursors WHERE bank_id = ?1 AND peer_id = ?2",
            params![bank_id, peer_label],
            |r| r.get(0),
        )
        .optional()
        .am("sync cursor get")
    })
}

/// SQLite body of `SyncStore::cursor_set`: upsert the peer's cursor token.
pub(crate) fn cursor_set_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    peer_label: &str,
    token: &str,
) -> Result<(), AmError> {
    let now = now_iso8601();
    store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sync_cursors (bank_id, peer_id, cursor_token, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?4) \
             ON CONFLICT (bank_id, peer_id) DO UPDATE SET \
             cursor_token = excluded.cursor_token, updated_at = excluded.updated_at",
            params![bank_id, peer_label, token, now],
        )
        .am("sync cursor set")?;
        Ok(())
    })
}

/// SQLite body of `SyncStore::conflict_insert`: append one LWW loser row.
pub(crate) fn conflict_insert_sqlite(
    store: &SqliteStore,
    row: &SyncConflictRow,
) -> Result<(), AmError> {
    store.with_conn(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO sync_conflicts (bank_id, entity_kind, entity_id, \
             winner_mutation_id, loser_mutation_id, winner_transaction_time, \
             loser_transaction_time, winner_device_id, loser_device_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                row.bank_id,
                row.entity_kind,
                row.entity_id,
                row.winner_mutation_id,
                row.loser_mutation_id,
                row.winner_transaction_time,
                row.loser_transaction_time,
                row.winner_device_id,
                row.loser_device_id,
                row.created_at,
            ],
        )
        .am("sync conflict insert")?;
        Ok(())
    })
}

/// SQLite body of `SyncStore::conflict_list`: newest losers for a bank.
pub(crate) fn conflict_list_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    limit: usize,
) -> Result<Vec<SyncConflictRow>, AmError> {
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CONFLICT_COLS} FROM sync_conflicts \
                     WHERE bank_id = ?1 ORDER BY created_at DESC, loser_mutation_id LIMIT ?2"
            ))
            .am("sync conflict list prepare")?;
        let mapped = stmt
            .query_map(params![bank_id, limit as i64], read_conflict_row)
            .am("sync conflict list query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync conflict list map")?;
        Ok(mapped)
    })
}

/// SQLite body of `SyncStore::conflict_count`: the bank's loser count.
pub(crate) fn conflict_count_sqlite(store: &SqliteStore, bank_id: &str) -> Result<i64, AmError> {
    store.with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE bank_id = ?1",
            params![bank_id],
            |r| r.get(0),
        )
        .am("sync conflict count")
    })
}

/// SQLite body of `SyncStore::dead_letter_open`: open/reopen a DLQ row.
pub(crate) fn dead_letter_open_sqlite(
    store: &SqliteStore,
    row: &SyncDeadLetterRow,
) -> Result<(), AmError> {
    let now = now_iso8601();
    store.with_conn(|conn| {
        let tx = conn.transaction().am("sync dlq begin")?;
        let first_seen: String = tx
            .query_row(
                "SELECT first_seen FROM sync_dead_letter WHERE sync_mutation_id = ?1",
                params![row.sync_mutation_id],
                |r| r.get(0),
            )
            .optional()
            .am("sync dlq first-seen probe")?
            .unwrap_or_else(|| row.first_seen.clone());
        let status = row.status.as_str();
        tx.execute(
            "INSERT INTO sync_dead_letter (sync_mutation_id, bank_id, reason_code, detail, \
             envelope_json, status, first_seen, last_error, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT (sync_mutation_id) DO UPDATE SET \
             reason_code = excluded.reason_code, detail = excluded.detail, \
             status = excluded.status, last_error = excluded.last_error, \
             updated_at = excluded.updated_at",
            params![
                row.sync_mutation_id,
                row.bank_id,
                row.reason_code,
                row.detail,
                row.envelope_json,
                status,
                first_seen,
                row.last_error,
                now,
            ],
        )
        .am("sync dlq upsert")?;
        tx.commit().am("sync dlq commit")?;
        Ok(())
    })
}

/// SQLite body of `SyncStore::dead_letter_status`: one DLQ row's status.
pub(crate) fn dead_letter_status_sqlite(
    store: &SqliteStore,
    sync_mutation_id: &str,
) -> Result<Option<DeadLetterStatus>, AmError> {
    store.with_conn(|conn| {
        conn.query_row(
            "SELECT status FROM sync_dead_letter WHERE sync_mutation_id = ?1",
            params![sync_mutation_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map(|status| status.as_deref().and_then(DeadLetterStatus::parse))
        .am("sync dlq status")
    })
}

/// SQLite body of `SyncStore::dead_letter_ack_skip`: ack an open DLQ row.
pub(crate) fn dead_letter_ack_skip_sqlite(
    store: &SqliteStore,
    sync_mutation_id: &str,
    at: &str,
) -> Result<bool, AmError> {
    store.with_conn(|conn| {
        let changed = conn
            .execute(
                "UPDATE sync_dead_letter SET status = 'acked_skip', updated_at = ?2 \
                 WHERE sync_mutation_id = ?1 AND status = 'open'",
                params![sync_mutation_id, at],
            )
            .am("sync dlq ack skip")?;
        Ok(changed > 0)
    })
}

/// SQLite body of `SyncStore::dead_letter_list`: DLQ rows in a status.
pub(crate) fn dead_letter_list_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    status: DeadLetterStatus,
    limit: usize,
) -> Result<Vec<SyncDeadLetterRow>, AmError> {
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {DLQ_COLS} FROM sync_dead_letter \
                     WHERE bank_id = ?1 AND status = ?2 ORDER BY first_seen LIMIT ?3"
            ))
            .am("sync dlq list prepare")?;
        let mapped = stmt
            .query_map(
                params![bank_id, status.as_str(), limit as i64],
                read_dlq_row,
            )
            .am("sync dlq list query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync dlq list map")?;
        Ok(mapped)
    })
}

/// SQLite body of `SyncStore::dead_letter_count`: DLQ rows in a status.
pub(crate) fn dead_letter_count_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    status: DeadLetterStatus,
) -> Result<i64, AmError> {
    store.with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM sync_dead_letter WHERE bank_id = ?1 AND status = ?2",
            params![bank_id, status.as_str()],
            |r| r.get(0),
        )
        .am("sync dlq count")
    })
}

/// SQLite body of `SyncStore::subject_is_shredded`: the erasure probe.
pub(crate) fn subject_is_shredded_sqlite(
    store: &SqliteStore,
    subject_id: &str,
) -> Result<bool, AmError> {
    store.with_conn(|conn| {
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM subject_keys WHERE subject_id = ?1",
                params![subject_id],
                |r| r.get(0),
            )
            .optional()
            .am("sync shredded probe")?;
        Ok(status.as_deref() == Some("shredded"))
    })
}

/// SQLite body of `SyncStore::persona_context_erased`: the bank-level
/// persona-context erasure probe (`persona_erase_tombstones`).
pub(crate) fn persona_context_erased_sqlite(
    store: &SqliteStore,
    bank_id: &str,
) -> Result<bool, AmError> {
    store.with_conn(|conn| {
        let hit: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM persona_erase_tombstones WHERE bank_id = ?1",
                params![bank_id],
                |r| r.get(0),
            )
            .optional()
            .am("sync persona erasure probe")?;
        Ok(hit.is_some())
    })
}
