// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres sync-protocol *state* SQL bodies
//! behind the `SyncStore` impl: the per-bank mutation
//! journal reads and guards, the opaque cursor tokens, the LWW conflict
//! journal, and the dead-letter quarantine.
//!
//! ## Owns
//! - Postgres SQL bodies for journal/cursor/conflict/dead-letter sync state
//!
//! ## Does not own
//! - The `SyncStore for PostgresStore` impl block (`postgres_sync`), which
//!   delegates to these bodies
//! - The journal *append* transaction and the change-feed SQL
//!   (`postgres_sync_records`)
//! - Task/failure history sync persistence (`postgres_sync_history`)
//! - SQLite sync state SQL (`sqlite_sync_state`)
//!
//! ## Boundary
//!
//! Changes to Postgres sync state SQL should normally require changes here.
//! Changes to apply or conflict policy should not.
//!
//! Keep this module focused on Postgres sync state persistence.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::postgres_sync_rows::{limit_i64, read_conflict_row, read_dlq_row, read_journal_row};
use crate::sync_store::{CONFLICT_COLS, DLQ_COLS, JOURNAL_COLS};
use crate::sync_store::{DeadLetterStatus, SyncConflictRow, SyncDeadLetterRow, SyncJournalRow};
use clio_types::{AmError, ErrorCode};

/// Postgres body of `SyncStore::journal_get`: one journal row by mutation id.
pub(crate) fn journal_get_pg(
    store: &PostgresStore,
    bank_id: &str,
    sync_mutation_id: &str,
) -> Result<Option<SyncJournalRow>, AmError> {
    store.with_client(|client| {
        let row = client
            .query_opt(
                &format!(
                    "SELECT {JOURNAL_COLS} FROM sync_mutations \
                     WHERE bank_id = $1 AND sync_mutation_id = $2"
                ),
                &[&bank_id, &sync_mutation_id],
            )
            .am("sync journal get")?;
        Ok(row.as_ref().map(read_journal_row))
    })
}

/// Postgres body of `SyncStore::journal_list_since`: journal rows past `seq`.
pub(crate) fn journal_list_since_pg(
    store: &PostgresStore,
    bank_id: &str,
    since_seq: i64,
    limit: usize,
) -> Result<Vec<SyncJournalRow>, AmError> {
    let since = i32::try_from(since_seq)
        .map_err(|_| AmError::new(ErrorCode::OutOfRange, "seq out of range"))?;
    store.with_client(|client| {
        let rows = client
            .query(
                &format!(
                    "SELECT {JOURNAL_COLS} FROM sync_mutations \
                     WHERE bank_id = $1 AND seq > $2 ORDER BY seq LIMIT $3"
                ),
                &[&bank_id, &since, &limit_i64(limit)],
            )
            .am("sync journal list")?;
        Ok(rows.iter().map(read_journal_row).collect())
    })
}

/// Postgres body of `SyncStore::journal_max_seq`: the per-bank high-water seq.
pub(crate) fn journal_max_seq_pg(store: &PostgresStore, bank_id: &str) -> Result<i64, AmError> {
    store.with_client(|client| {
        let max: i32 = client
            .query_one(
                "SELECT COALESCE(MAX(seq), 0) FROM sync_mutations WHERE bank_id = $1",
                &[&bank_id],
            )
            .am("sync journal max seq")?
            .get(0);
        Ok(i64::from(max))
    })
}

/// Postgres body of `SyncStore::journal_apply_guard_hit`: idempotency probe.
pub(crate) fn journal_apply_guard_hit_pg(
    store: &PostgresStore,
    bank_id: &str,
    entity_kind: &str,
    entity_id: &str,
    op: &str,
    content_hash: &str,
) -> Result<bool, AmError> {
    store.with_client(|client| {
        let row = client
            .query_opt(
                "SELECT 1 FROM sync_mutations WHERE bank_id = $1 AND entity_kind = $2 \
                 AND entity_id = $3 AND op = $4 AND content_hash = $5 LIMIT 1",
                &[&bank_id, &entity_kind, &entity_id, &op, &content_hash],
            )
            .am("sync journal apply guard")?;
        Ok(row.is_some())
    })
}

/// Postgres body of `SyncStore::cursor_get`: the peer's opaque cursor token.
pub(crate) fn cursor_get_pg(
    store: &PostgresStore,
    bank_id: &str,
    peer_label: &str,
) -> Result<Option<String>, AmError> {
    store.with_client(|client| {
        let row = client
            .query_opt(
                "SELECT cursor_token FROM sync_cursors WHERE bank_id = $1 AND peer_id = $2",
                &[&bank_id, &peer_label],
            )
            .am("sync cursor get")?;
        Ok(row.map(|r| r.get(0)))
    })
}

/// Postgres body of `SyncStore::cursor_set`: upsert the peer's cursor token.
pub(crate) fn cursor_set_pg(
    store: &PostgresStore,
    bank_id: &str,
    peer_label: &str,
    token: &str,
) -> Result<(), AmError> {
    let now = crate::migrate::now_iso8601();
    store.with_client(|client| {
        client
            .execute(
                "INSERT INTO sync_cursors (bank_id, peer_id, cursor_token, created_at, \
                 updated_at) VALUES ($1, $2, $3, $4, $4) \
                 ON CONFLICT (bank_id, peer_id) DO UPDATE SET \
                 cursor_token = excluded.cursor_token, updated_at = excluded.updated_at",
                &[&bank_id, &peer_label, &token, &now],
            )
            .am("sync cursor set")?;
        Ok(())
    })
}

/// Postgres body of `SyncStore::conflict_insert`: append one LWW loser row.
pub(crate) fn conflict_insert_pg(
    store: &PostgresStore,
    row: &SyncConflictRow,
) -> Result<(), AmError> {
    store.with_client(|client| {
        client
            .execute(
                "INSERT INTO sync_conflicts (bank_id, entity_kind, entity_id, \
                 winner_mutation_id, loser_mutation_id, winner_transaction_time, \
                 loser_transaction_time, winner_device_id, loser_device_id, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
                 ON CONFLICT (bank_id, loser_mutation_id) DO NOTHING",
                &[
                    &row.bank_id,
                    &row.entity_kind,
                    &row.entity_id,
                    &row.winner_mutation_id,
                    &row.loser_mutation_id,
                    &row.winner_transaction_time,
                    &row.loser_transaction_time,
                    &row.winner_device_id,
                    &row.loser_device_id,
                    &row.created_at,
                ],
            )
            .am("sync conflict insert")?;
        Ok(())
    })
}

/// Postgres body of `SyncStore::conflict_list`: newest losers for a bank.
pub(crate) fn conflict_list_pg(
    store: &PostgresStore,
    bank_id: &str,
    limit: usize,
) -> Result<Vec<SyncConflictRow>, AmError> {
    store.with_client(|client| {
        let rows = client
            .query(
                &format!(
                    "SELECT {CONFLICT_COLS} FROM sync_conflicts \
                     WHERE bank_id = $1 ORDER BY created_at DESC, loser_mutation_id LIMIT $2"
                ),
                &[&bank_id, &limit_i64(limit)],
            )
            .am("sync conflict list")?;
        Ok(rows.iter().map(read_conflict_row).collect())
    })
}

/// Postgres body of `SyncStore::conflict_count`: the bank's loser count.
pub(crate) fn conflict_count_pg(store: &PostgresStore, bank_id: &str) -> Result<i64, AmError> {
    store.with_client(|client| {
        let n: i64 = client
            .query_one(
                "SELECT COUNT(*)::bigint FROM sync_conflicts WHERE bank_id = $1",
                &[&bank_id],
            )
            .am("sync conflict count")?
            .get(0);
        Ok(n)
    })
}

/// Postgres body of `SyncStore::dead_letter_open`: open/reopen a DLQ row.
pub(crate) fn dead_letter_open_pg(
    store: &PostgresStore,
    row: &SyncDeadLetterRow,
) -> Result<(), AmError> {
    let now = crate::migrate::now_iso8601();
    store.with_client(|client| {
        client
            .execute(
                "INSERT INTO sync_dead_letter (sync_mutation_id, bank_id, reason_code, \
                 detail, envelope_json, status, first_seen, last_error, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                 ON CONFLICT (sync_mutation_id) DO UPDATE SET \
                 bank_id = excluded.bank_id, \
                 reason_code = excluded.reason_code, detail = excluded.detail, \
                 status = excluded.status, last_error = excluded.last_error, \
                 updated_at = excluded.updated_at",
                &[
                    &row.sync_mutation_id,
                    &row.bank_id,
                    &row.reason_code,
                    &row.detail,
                    &row.envelope_json,
                    &row.status.as_str(),
                    &row.first_seen,
                    &row.last_error,
                    &now,
                ],
            )
            .am("sync dlq open")?;
        Ok(())
    })
}

/// Postgres body of `SyncStore::dead_letter_status`: one DLQ row's status.
pub(crate) fn dead_letter_status_pg(
    store: &PostgresStore,
    sync_mutation_id: &str,
) -> Result<Option<DeadLetterStatus>, AmError> {
    store.with_client(|client| {
        let row = client
            .query_opt(
                "SELECT status FROM sync_dead_letter WHERE sync_mutation_id = $1",
                &[&sync_mutation_id],
            )
            .am("sync dlq status")?;
        Ok(row.map(|r| DeadLetterStatus::parse(r.get(0)).unwrap_or(DeadLetterStatus::Open)))
    })
}

/// Postgres body of `SyncStore::dead_letter_ack_skip`: ack an open DLQ row.
pub(crate) fn dead_letter_ack_skip_pg(
    store: &PostgresStore,
    sync_mutation_id: &str,
    at: &str,
) -> Result<bool, AmError> {
    store.with_client(|client| {
        let n = client
            .execute(
                "UPDATE sync_dead_letter SET status = 'acked_skip', updated_at = $2 \
                 WHERE sync_mutation_id = $1 AND status = 'open'",
                &[&sync_mutation_id, &at],
            )
            .am("sync dlq ack skip")?;
        Ok(n > 0)
    })
}

/// Postgres body of `SyncStore::dead_letter_list`: DLQ rows in a status.
pub(crate) fn dead_letter_list_pg(
    store: &PostgresStore,
    bank_id: &str,
    status: DeadLetterStatus,
    limit: usize,
) -> Result<Vec<SyncDeadLetterRow>, AmError> {
    store.with_client(|client| {
        let rows = client
            .query(
                &format!(
                    "SELECT {DLQ_COLS} FROM sync_dead_letter \
                     WHERE bank_id = $1 AND status = $2 ORDER BY first_seen LIMIT $3"
                ),
                &[&bank_id, &status.as_str(), &limit_i64(limit)],
            )
            .am("sync dlq list")?;
        Ok(rows.iter().map(read_dlq_row).collect())
    })
}

/// Postgres body of `SyncStore::dead_letter_count`: DLQ rows in a status.
pub(crate) fn dead_letter_count_pg(
    store: &PostgresStore,
    bank_id: &str,
    status: DeadLetterStatus,
) -> Result<i64, AmError> {
    store.with_client(|client| {
        let n: i64 = client
            .query_one(
                "SELECT COUNT(*)::bigint FROM sync_dead_letter \
                 WHERE bank_id = $1 AND status = $2",
                &[&bank_id, &status.as_str()],
            )
            .am("sync dlq count")?
            .get(0);
        Ok(n)
    })
}

/// Postgres body of `SyncStore::subject_is_shredded`: the erasure probe.
pub(crate) fn subject_is_shredded_pg(
    store: &PostgresStore,
    subject_id: &str,
) -> Result<bool, AmError> {
    store.with_client(|client| {
        let row = client
            .query_opt(
                "SELECT status FROM subject_keys WHERE subject_id = $1",
                &[&subject_id],
            )
            .am("sync shredded probe")?;
        Ok(row.is_some_and(|r| r.get::<_, String>(0) == "shredded"))
    })
}

/// Postgres body of `SyncStore::persona_context_erased`: the bank-level
/// persona-context erasure probe (`persona_erase_tombstones`).
pub(crate) fn persona_context_erased_pg(
    store: &PostgresStore,
    bank_id: &str,
) -> Result<bool, AmError> {
    store.with_client(|client| {
        let row = client
            .query_opt(
                "SELECT 1 FROM persona_erase_tombstones WHERE bank_id = $1",
                &[&bank_id],
            )
            .am("sync persona erasure probe")?;
        Ok(row.is_some())
    })
}
