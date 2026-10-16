// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two driver halves of the durable
//! per-peer action contract: the SQLite and Postgres SQL for the idempotent
//! begin, the guarded advance, and the restart reads.
//!
//! ## Owns
//! - `PeerActionStore for SqliteStore` (immediate read-then-write transactions)
//! - `PeerActionStore for PostgresStore` (one transaction per call, a
//!   single-row lock while a row is settled, and a re-read after the insert)
//!
//! ## Does not own
//! - The contract itself: the row types, the trait, the transition table,
//!   validation, and the fail-closed decoding (`peer_actions` owns those)
//! - The `peer_actions` DDL text (`sql/003_local_compliance_slots.sql`)
//!
//! ## Boundary
//!
//! Changes to a driver's SQL should normally require changes here. Changes to
//! the row shape or the transition rules should not.
//!
//! Keep this module focused on the two drivers' SQL.

use super::{
    COLUMNS, PeerActionBinding, PeerActionKind, PeerActionRow, PeerActionStore, PeerActionTarget,
    StoredRow, binding_conflict, illegal_transition, lost_row, same_state_is_no_op,
    transition_legal, unknown_action, validate_binding, validate_target,
};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use clio_types::AmError;
use rusqlite::OptionalExtension;

fn sqlite_stored(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRow> {
    Ok(StoredRow {
        request_id: row.get(0)?,
        peer_id: row.get(1)?,
        action_kind: row.get(2)?,
        action_id: row.get(3)?,
        state: row.get(4)?,
        disposition: row.get(5)?,
        proof_ref: row.get(6)?,
    })
}

fn sqlite_read(
    conn: &rusqlite::Connection,
    request_id: &str,
    peer_id: &str,
    action_kind: PeerActionKind,
) -> Result<Option<PeerActionRow>, AmError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM peer_actions
         WHERE request_id = ?1 AND peer_id = ?2 AND action_kind = ?3"
    );
    conn.query_row(
        &sql,
        rusqlite::params![request_id, peer_id, action_kind.as_db()],
        sqlite_stored,
    )
    .optional()
    .am("peer action read")?
    .map(StoredRow::decode)
    .transpose()
}

impl PeerActionStore for SqliteStore {
    fn begin(&self, binding: &PeerActionBinding<'_>) -> Result<PeerActionRow, AmError> {
        validate_binding(binding)?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .am("peer action begin")?;
            tx.execute(
                "INSERT INTO peer_actions
                    (request_id, peer_id, action_kind, action_id, state, disposition, proof_ref)
                 VALUES (?1, ?2, ?3, ?4, 'pending', 'unknown', NULL)
                 ON CONFLICT (request_id, peer_id, action_kind) DO NOTHING",
                rusqlite::params![
                    binding.request_id,
                    binding.peer_id,
                    binding.action_kind.as_db(),
                    binding.action_id
                ],
            )
            .am("peer action insert")?;
            // Re-read inside the same transaction: a skipped insert may belong
            // to another writer, so who inserted is never inferred from it.
            let stored = sqlite_read(
                &tx,
                binding.request_id,
                binding.peer_id,
                binding.action_kind,
            )?
            .ok_or_else(lost_row)?;
            if stored.action_id != binding.action_id {
                return Err(binding_conflict(binding));
            }
            tx.commit().am("peer action commit")?;
            Ok(stored)
        })
    }

    fn advance(
        &self,
        binding: &PeerActionBinding<'_>,
        target: &PeerActionTarget<'_>,
    ) -> Result<PeerActionRow, AmError> {
        validate_binding(binding)?;
        validate_target(target)?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .am("peer action advance")?;
            let stored = sqlite_read(
                &tx,
                binding.request_id,
                binding.peer_id,
                binding.action_kind,
            )?
            .ok_or_else(|| unknown_action(binding))?;
            if stored.action_id != binding.action_id {
                return Err(binding_conflict(binding));
            }
            if stored.state == target.to {
                // The state is unchanged: either the tolerated no-op a
                // duplicate delivery or acknowledgement is, or the one
                // disposition a refused row still owes is recorded here.
                if same_state_is_no_op(&stored, target) {
                    tx.commit().am("peer action commit")?;
                    return Ok(stored);
                }
            } else if !transition_legal(stored.state, target.to) {
                return Err(illegal_transition(stored.state, target.to));
            }
            tx.execute(
                "UPDATE peer_actions
                 SET state = ?1, disposition = ?2, proof_ref = ?3
                 WHERE request_id = ?4 AND peer_id = ?5
                   AND action_kind = ?6 AND action_id = ?7",
                rusqlite::params![
                    target.to.as_db(),
                    target.disposition.as_db(),
                    target.proof_ref,
                    binding.request_id,
                    binding.peer_id,
                    binding.action_kind.as_db(),
                    binding.action_id
                ],
            )
            .am("peer action update")?;
            let settled = sqlite_read(
                &tx,
                binding.request_id,
                binding.peer_id,
                binding.action_kind,
            )?
            .ok_or_else(lost_row)?;
            tx.commit().am("peer action commit")?;
            Ok(settled)
        })
    }

    fn load(
        &self,
        request_id: &str,
        peer_id: &str,
        action_kind: PeerActionKind,
    ) -> Result<Option<PeerActionRow>, AmError> {
        self.with_conn(|conn| sqlite_read(conn, request_id, peer_id, action_kind))
    }

    fn load_for_request(&self, request_id: &str) -> Result<Vec<PeerActionRow>, AmError> {
        self.with_conn(|conn| {
            let sql = format!(
                "SELECT {COLUMNS} FROM peer_actions
                 WHERE request_id = ?1 ORDER BY peer_id, action_kind"
            );
            let mut stmt = conn.prepare(&sql).am("peer action list prepare")?;
            let rows = stmt
                .query_map(rusqlite::params![request_id], sqlite_stored)
                .am("peer action list rows")?;
            rows.collect::<Result<Vec<_>, _>>()
                .am("peer action list rows")?
                .into_iter()
                .map(StoredRow::decode)
                .collect()
        })
    }
}

fn pg_stored(row: &postgres::Row) -> StoredRow {
    StoredRow {
        request_id: row.get(0),
        peer_id: row.get(1),
        action_kind: row.get(2),
        action_id: row.get(3),
        state: row.get(4),
        disposition: row.get(5),
        proof_ref: row.get(6),
    }
}

/// The one-row read; `lock` takes the single-row lock that holds the peer row
/// while it is settled.
fn pg_read_sql(lock: bool) -> String {
    format!(
        "SELECT {COLUMNS} FROM peer_actions
         WHERE request_id = $1 AND peer_id = $2 AND action_kind = $3{}",
        if lock { " FOR UPDATE" } else { "" }
    )
}

/// Decode one optional read result, failing closed on unknown spellings.
fn pg_decode(row: Option<postgres::Row>) -> Result<Option<PeerActionRow>, AmError> {
    row.map(|row| pg_stored(&row).decode()).transpose()
}

impl PeerActionStore for PostgresStore {
    fn begin(&self, binding: &PeerActionBinding<'_>) -> Result<PeerActionRow, AmError> {
        validate_binding(binding)?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("peer action begin")?;
            tx.execute(
                "INSERT INTO peer_actions
                    (request_id, peer_id, action_kind, action_id, state, disposition, proof_ref)
                 VALUES ($1, $2, $3, $4, 'pending', 'unknown', NULL)
                 ON CONFLICT (request_id, peer_id, action_kind) DO NOTHING",
                &[
                    &binding.request_id,
                    &binding.peer_id,
                    &binding.action_kind.as_db(),
                    &binding.action_id,
                ],
            )
            .am("peer action insert")?;
            // Re-read inside the same transaction: a skipped insert may belong
            // to another writer, so who inserted is never inferred from it.
            let stored = pg_decode(
                tx.query_opt(
                    &pg_read_sql(false),
                    &[
                        &binding.request_id,
                        &binding.peer_id,
                        &binding.action_kind.as_db(),
                    ],
                )
                .am("peer action read")?,
            )?
            .ok_or_else(lost_row)?;
            if stored.action_id != binding.action_id {
                return Err(binding_conflict(binding));
            }
            tx.commit().am("peer action commit")?;
            Ok(stored)
        })
    }

    fn advance(
        &self,
        binding: &PeerActionBinding<'_>,
        target: &PeerActionTarget<'_>,
    ) -> Result<PeerActionRow, AmError> {
        validate_binding(binding)?;
        validate_target(target)?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("peer action advance")?;
            // The single-row lock holds the peer row while it is settled.
            let stored = pg_decode(
                tx.query_opt(
                    &pg_read_sql(true),
                    &[
                        &binding.request_id,
                        &binding.peer_id,
                        &binding.action_kind.as_db(),
                    ],
                )
                .am("peer action read")?,
            )?
            .ok_or_else(|| unknown_action(binding))?;
            if stored.action_id != binding.action_id {
                return Err(binding_conflict(binding));
            }
            if stored.state == target.to {
                // The state is unchanged: either the tolerated no-op a
                // duplicate delivery or acknowledgement is, or the one
                // disposition a refused row still owes is recorded here.
                if same_state_is_no_op(&stored, target) {
                    tx.commit().am("peer action commit")?;
                    return Ok(stored);
                }
            } else if !transition_legal(stored.state, target.to) {
                return Err(illegal_transition(stored.state, target.to));
            }
            tx.execute(
                "UPDATE peer_actions
                 SET state = $1, disposition = $2, proof_ref = $3
                 WHERE request_id = $4 AND peer_id = $5
                   AND action_kind = $6 AND action_id = $7",
                &[
                    &target.to.as_db(),
                    &target.disposition.as_db(),
                    &target.proof_ref,
                    &binding.request_id,
                    &binding.peer_id,
                    &binding.action_kind.as_db(),
                    &binding.action_id,
                ],
            )
            .am("peer action update")?;
            let settled = pg_decode(
                tx.query_opt(
                    &pg_read_sql(false),
                    &[
                        &binding.request_id,
                        &binding.peer_id,
                        &binding.action_kind.as_db(),
                    ],
                )
                .am("peer action read")?,
            )?
            .ok_or_else(lost_row)?;
            tx.commit().am("peer action commit")?;
            Ok(settled)
        })
    }

    fn load(
        &self,
        request_id: &str,
        peer_id: &str,
        action_kind: PeerActionKind,
    ) -> Result<Option<PeerActionRow>, AmError> {
        self.with_client(|client| {
            pg_decode(
                client
                    .query_opt(
                        &pg_read_sql(false),
                        &[&request_id, &peer_id, &action_kind.as_db()],
                    )
                    .am("peer action read")?,
            )
        })
    }

    fn load_for_request(&self, request_id: &str) -> Result<Vec<PeerActionRow>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query(
                    &format!(
                        "SELECT {COLUMNS} FROM peer_actions
                         WHERE request_id = $1 ORDER BY peer_id, action_kind"
                    ),
                    &[&request_id],
                )
                .am("peer action list rows")?;
            rows.iter().map(|row| pg_stored(row).decode()).collect()
        })
    }
}
