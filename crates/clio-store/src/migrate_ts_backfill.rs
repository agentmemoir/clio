// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for rewriting item timestamps a previous build
//! stored as epoch millis into the canonical RFC 3339 form the item time
//! columns are compared in, on both storage backends.
//!
//! ## Owns
//! - `backfill_sqlite` / `backfill_postgres`: the per-backend rewrite pass
//! - `fixed_stamps`: the one rule deciding the value of each rewritten column
//!
//! ## Does not own
//! - The canonical form itself (`migrate::canonical_ts`) or the schema DDL
//! - When the pass runs (each backend's open path) or its transaction boundary
//!
//! ## Boundary
//!
//! Changes to which stored value wins for a rewritten column belong here.
//! Changes to the canonical form do not.
//!
//! The pass only ever touches all-digit values, so running it on a database it
//! already rewrote changes nothing. Every value it decides on is normalised
//! before it is read, so the row order the backend happens to return cannot
//! change the answer.

use crate::migrate::{IntoAmError, canonical_ts, is_epoch_stamp};
use clio_types::AmError;

/// Every item time column the pass rewrites, in the order each statement names
/// them.
///
/// The first four are `NOT NULL`; the three that close a row's window are
/// nullable, and are compared as text by the same reads, so a millis value in
/// one of them is as unreadable as a millis creation time. A `NULL` stays a
/// `NULL`: an empty string would sort before every real stamp, which is the
/// failure this pass exists to remove.
const TIME_COLUMNS: [&str; 7] = [
    "created_at",
    "valid_from",
    "tx_from",
    "updated_at",
    "valid_until",
    "tx_until",
    "discarded_at",
];

/// One item row the pass read, in canonical-or-not form.
pub(crate) struct StoredStamps {
    /// Item id.
    pub id: String,
    /// The bank the row belongs to, which scopes every lookup keyed by id.
    pub bank_id: String,
    /// Stored `created_at`.
    pub created_at: String,
    /// Stored `valid_from`.
    pub valid_from: String,
    /// Stored `tx_from`.
    pub tx_from: String,
    /// Stored `updated_at`.
    pub updated_at: String,
    /// Stored `valid_until`, absent when the row has no upper bound.
    pub valid_until: Option<String>,
    /// Stored `tx_until`, absent when the row was never superseded.
    pub tx_until: Option<String>,
    /// Stored `discarded_at`, absent while the row is in active use.
    pub discarded_at: Option<String>,
    /// The hub the item was distilled from, when the item is a distilled unit.
    pub hub_id: Option<String>,
}

/// The replacement value of every time column, plus how many actually moved.
pub(crate) struct FixedStamps {
    /// Canonical `created_at`.
    pub created_at: String,
    /// Canonical `valid_from`.
    pub valid_from: String,
    /// Canonical `tx_from`.
    pub tx_from: String,
    /// Canonical `updated_at`.
    pub updated_at: String,
    /// Canonical `valid_until`, still absent when the column was `NULL`.
    pub valid_until: Option<String>,
    /// Canonical `tx_until`, still absent when the column was `NULL`.
    pub tx_until: Option<String>,
    /// Canonical `discarded_at`, still absent when the column was `NULL`.
    pub discarded_at: Option<String>,
    /// Whether any column changed.
    pub changed: bool,
}

/// The canonical form of one row's time columns.
///
/// `created_at` prefers the creation time of the item this one was distilled
/// from: a distilled unit is a summary of a cluster that existed before it, so
/// the moment it summarizes is the hub's own creation, not the moment the
/// summary was written. `hub_created_at` is that source's stored value, already
/// read by the caller, and it is normalised here before it is used. That is what
/// makes the rule independent of row order: a hub this pass has not rewritten
/// yet still contributes the instant its stored value names, so a unit visited
/// first inherits the same time as a unit visited last.
///
/// The other columns are the distillation moment and the row's own window by
/// definition, so they are converted from their own stored value.
pub(crate) fn fixed_stamps(row: &StoredStamps, hub_created_at: Option<&str>) -> FixedStamps {
    let created_at = hub_created_at.map_or_else(|| canonical_ts(&row.created_at), canonical_ts);
    let valid_from = canonical_ts(&row.valid_from);
    let tx_from = canonical_ts(&row.tx_from);
    let updated_at = canonical_ts(&row.updated_at);
    let valid_until = window_stamp(row.valid_until.as_deref());
    let tx_until = window_stamp(row.tx_until.as_deref());
    let discarded_at = window_stamp(row.discarded_at.as_deref());
    let changed = created_at != row.created_at
        || valid_from != row.valid_from
        || tx_from != row.tx_from
        || updated_at != row.updated_at
        || valid_until != row.valid_until
        || tx_until != row.tx_until
        || discarded_at != row.discarded_at;
    FixedStamps {
        created_at,
        valid_from,
        tx_from,
        updated_at,
        valid_until,
        tx_until,
        discarded_at,
        changed,
    }
}

/// A nullable window column's replacement value: `NULL` stays `NULL`, and any
/// stored value is normalised like every other.
fn window_stamp(raw: Option<&str>) -> Option<String> {
    raw.map(canonical_ts)
}

/// Whether any of the row's time columns is still an epoch stamp.
fn needs_rewrite(row: &StoredStamps) -> bool {
    [
        Some(row.created_at.as_str()),
        Some(row.valid_from.as_str()),
        Some(row.tx_from.as_str()),
        Some(row.updated_at.as_str()),
        row.valid_until.as_deref(),
        row.tx_until.as_deref(),
        row.discarded_at.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(is_epoch_stamp)
}

/// Rewrite epoch-millis item timestamps on a SQLite database.
///
/// Returns how many rows were rewritten. A database with nothing to fix runs one
/// filtered `SELECT` and returns zero. The connection is taken immutably because
/// every statement here runs inside the caller's transaction: a failure partway
/// through this pass rolls back with it.
///
/// # Errors
/// Storage failures on the read or on a rewrite.
pub(crate) fn backfill_sqlite(conn: &rusqlite::Connection) -> Result<usize, AmError> {
    let candidates = {
        let mut statement = conn
            .prepare(&candidate_query_sqlite())
            .am("timestamp backfill read")?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredStamps {
                    id: row.get(0)?,
                    bank_id: row.get(1)?,
                    created_at: row.get(2)?,
                    valid_from: row.get(3)?,
                    tx_from: row.get(4)?,
                    updated_at: row.get(5)?,
                    valid_until: row.get(6)?,
                    tx_until: row.get(7)?,
                    discarded_at: row.get(8)?,
                    hub_id: row.get(9)?,
                })
            })
            .am("timestamp backfill read")?;
        let mut candidates = Vec::new();
        for row in rows {
            let row = row.am("timestamp backfill row")?;
            if needs_rewrite(&row) {
                candidates.push(row);
            }
        }
        candidates
    };
    let mut rewritten = 0;
    for row in candidates {
        let hub_created_at = match &row.hub_id {
            Some(hub_id) => conn
                .query_row(
                    "SELECT created_at FROM items WHERE id = ?1 AND bank_id = ?2",
                    [hub_id, &row.bank_id],
                    |r| r.get::<_, String>(0),
                )
                .ok(),
            None => None,
        };
        let fixed = fixed_stamps(&row, hub_created_at.as_deref());
        if !fixed.changed {
            continue;
        }
        conn.execute(
            "UPDATE items SET created_at = ?3, valid_from = ?4, tx_from = ?5, updated_at = ?6,
                    valid_until = ?7, tx_until = ?8, discarded_at = ?9
             WHERE id = ?1 AND bank_id = ?2",
            rusqlite::params![
                row.id,
                row.bank_id,
                fixed.created_at,
                fixed.valid_from,
                fixed.tx_from,
                fixed.updated_at,
                fixed.valid_until,
                fixed.tx_until,
                fixed.discarded_at
            ],
        )
        .am("timestamp backfill write")?;
        rewritten += 1;
    }
    Ok(rewritten)
}

/// Rewrite epoch-millis item timestamps on a Postgres database.
///
/// Returns how many rows were rewritten. A database with nothing to fix runs one
/// filtered `SELECT` and returns zero. The whole pass runs in one transaction, so
/// a failure partway through leaves the table as it was found.
///
/// # Errors
/// Storage failures on the read or on a rewrite.
pub(crate) fn backfill_postgres(client: &mut postgres::Client) -> Result<usize, AmError> {
    client
        .batch_execute("BEGIN")
        .am("timestamp backfill begin")?;
    match rewrite_postgres(client) {
        Ok(rewritten) => {
            client
                .batch_execute("COMMIT")
                .am("timestamp backfill commit")?;
            Ok(rewritten)
        }
        Err(error) => {
            client
                .batch_execute("ROLLBACK")
                .am("timestamp backfill rollback")?;
            Err(error)
        }
    }
}

/// The Postgres rewrite inside [`backfill_postgres`]'s transaction.
fn rewrite_postgres(client: &mut postgres::Client) -> Result<usize, AmError> {
    let rows = client
        .query(&candidate_query_postgres(), &[])
        .am("timestamp backfill read")?;
    let mut rewritten = 0;
    for row in rows {
        let stored = StoredStamps {
            id: row.get(0),
            bank_id: row.get(1),
            created_at: row.get(2),
            valid_from: row.get(3),
            tx_from: row.get(4),
            updated_at: row.get(5),
            valid_until: row.get(6),
            tx_until: row.get(7),
            discarded_at: row.get(8),
            hub_id: row.get(9),
        };
        if !needs_rewrite(&stored) {
            continue;
        }
        let hub_created_at = match &stored.hub_id {
            Some(hub_id) => client
                .query_opt(
                    "SELECT created_at FROM items WHERE id = $1 AND bank_id = $2",
                    &[hub_id, &stored.bank_id],
                )
                .ok()
                .flatten()
                .map(|r| r.get::<_, String>(0)),
            None => None,
        };
        let fixed = fixed_stamps(&stored, hub_created_at.as_deref());
        if !fixed.changed {
            continue;
        }
        client
            .execute(
                "UPDATE items SET created_at = $3, valid_from = $4, tx_from = $5,
                        updated_at = $6, valid_until = $7, tx_until = $8, discarded_at = $9
                 WHERE id = $1 AND bank_id = $2",
                &[
                    &stored.id,
                    &stored.bank_id,
                    &fixed.created_at,
                    &fixed.valid_from,
                    &fixed.tx_from,
                    &fixed.updated_at,
                    &fixed.valid_until,
                    &fixed.tx_until,
                    &fixed.discarded_at,
                ],
            )
            .am("timestamp backfill write")?;
        rewritten += 1;
    }
    Ok(rewritten)
}

/// The candidate read for the SQLite backend.
fn candidate_query_sqlite() -> String {
    let epoch = TIME_COLUMNS
        .iter()
        .map(|column| format!("({column} NOT GLOB '*[^0-9]*' AND {column} <> '')"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "SELECT id, bank_id, created_at, valid_from, tx_from, updated_at,
                valid_until, tx_until, discarded_at, {}
         FROM items WHERE {epoch}",
        hub_column()
    )
}

/// The candidate read for the Postgres backend.
fn candidate_query_postgres() -> String {
    let epoch = TIME_COLUMNS
        .iter()
        .map(|column| format!("{column} ~ '^[0-9]+$'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "SELECT id, bank_id, created_at, valid_from, tx_from, updated_at,
                valid_until, tx_until, discarded_at, {}
         FROM items WHERE {epoch}",
        hub_column()
    )
}

/// The hub a row was distilled from, when the row is a distilled unit.
///
/// A distilled unit's snapshot holds no `hub_id` of its own: the snapshot is
/// sealed inside the row's content, and the derivation record is what projects
/// it. The record's own `hub_id` is that value in plaintext, so the pass reads it
/// from there. It is resolved together with the row's own `bank_id`, so an id
/// shared by two banks can never hand one bank's creation time to the other.
fn hub_column() -> String {
    "(SELECT hub_id FROM distilled_units d WHERE d.item_id = items.id AND d.bank_id = items.bank_id)"
        .to_owned()
}

#[cfg(test)]
#[path = "migrate_ts_backfill_tests.rs"]
mod migrate_ts_backfill_tests;
