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
//! - When the pass runs (each backend's open path)
//!
//! ## Boundary
//!
//! Changes to which stored value wins for a rewritten column belong here.
//! Changes to the canonical form do not.
//!
//! The pass only ever touches all-digit values, so running it on a database it
//! already rewrote changes nothing.

use crate::migrate::{IntoAmError, canonical_ts, is_epoch_stamp};
use clio_types::AmError;

/// The four item time columns, in the order every statement names them.
const TIME_COLUMNS: [&str; 4] = ["created_at", "valid_from", "tx_from", "updated_at"];

/// One item row the pass read, in canonical-or-not form.
pub(crate) struct StoredStamps {
    /// Item id.
    pub id: String,
    /// Stored `created_at`.
    pub created_at: String,
    /// Stored `valid_from`.
    pub valid_from: String,
    /// Stored `tx_from`.
    pub tx_from: String,
    /// Stored `updated_at`.
    pub updated_at: String,
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
    /// Whether any column changed.
    pub changed: bool,
}

/// The canonical form of one row's four time columns.
///
/// `created_at` prefers the creation time of the item this one was distilled
/// from: a distilled unit is a summary of a cluster that existed before it, so
/// the moment it summarizes is the hub's own creation, not the moment the
/// summary was written. `hub_created_at` is that source's stored value, already
/// read by the caller, and is used only when it is not itself an epoch stamp;
/// otherwise the column's own millis value is converted, because a source time
/// this build cannot read is no better than the value being replaced.
///
/// The other three columns are the distillation moment by definition, so they
/// are converted from their own stored value.
pub(crate) fn fixed_stamps(row: &StoredStamps, hub_created_at: Option<&str>) -> FixedStamps {
    let created_at = hub_created_at
        .filter(|value| !is_epoch_stamp(value))
        .map_or_else(|| canonical_ts(&row.created_at), canonical_ts);
    let valid_from = canonical_ts(&row.valid_from);
    let tx_from = canonical_ts(&row.tx_from);
    let updated_at = canonical_ts(&row.updated_at);
    let changed = created_at != row.created_at
        || valid_from != row.valid_from
        || tx_from != row.tx_from
        || updated_at != row.updated_at;
    FixedStamps {
        created_at,
        valid_from,
        tx_from,
        updated_at,
        changed,
    }
}

/// Rewrite epoch-millis item timestamps on a SQLite database.
///
/// Returns how many rows were rewritten. A database with nothing to fix runs
/// one filtered `SELECT` and returns zero.
///
/// # Errors
/// Storage failures on the read or on a rewrite.
pub(crate) fn backfill_sqlite(conn: &mut rusqlite::Connection) -> Result<usize, AmError> {
    let epoch = TIME_COLUMNS
        .iter()
        .map(|column| format!("({column} NOT GLOB '*[^0-9]*' AND {column} <> '')"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let query = format!(
        "SELECT id, created_at, valid_from, tx_from, updated_at, {} FROM items WHERE {epoch}",
        hub_column()
    );
    let candidates = {
        let mut statement = conn.prepare(&query).am("timestamp backfill read")?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredStamps {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    valid_from: row.get(2)?,
                    tx_from: row.get(3)?,
                    updated_at: row.get(4)?,
                    hub_id: row.get(5)?,
                })
            })
            .am("timestamp backfill read")?;
        let mut candidates = Vec::new();
        for row in rows {
            let row = row.am("timestamp backfill row")?;
            if TIME_COLUMNS
                .iter()
                .any(|column| is_epoch_stamp(stored_column(&row, column)))
            {
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
                    "SELECT created_at FROM items WHERE id = ?1 AND bank_id = (
                       SELECT bank_id FROM items WHERE id = ?1)",
                    [hub_id],
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
            "UPDATE items SET created_at = ?2, valid_from = ?3, tx_from = ?4, updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![
                row.id,
                fixed.created_at,
                fixed.valid_from,
                fixed.tx_from,
                fixed.updated_at
            ],
        )
        .am("timestamp backfill write")?;
        rewritten += 1;
    }
    Ok(rewritten)
}

/// Rewrite epoch-millis item timestamps on a Postgres database.
///
/// Returns how many rows were rewritten. A database with nothing to fix runs
/// one filtered `SELECT` and returns zero.
///
/// # Errors
/// Storage failures on the read or on a rewrite.
pub(crate) fn backfill_postgres(client: &mut postgres::Client) -> Result<usize, AmError> {
    let epoch = TIME_COLUMNS
        .iter()
        .map(|column| format!("{column} ~ '^[0-9]+$'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let query = format!(
        "SELECT id, created_at, valid_from, tx_from, updated_at, {} FROM items WHERE {epoch}",
        hub_column()
    );
    let rows = client.query(&query, &[]).am("timestamp backfill read")?;
    let mut rewritten = 0;
    for row in rows {
        let stored = StoredStamps {
            id: row.get(0),
            created_at: row.get(1),
            valid_from: row.get(2),
            tx_from: row.get(3),
            updated_at: row.get(4),
            hub_id: row.get(5),
        };
        let hub_created_at = match &stored.hub_id {
            Some(hub_id) => client
                .query_opt("SELECT created_at FROM items WHERE id = $1", &[hub_id])
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
                "UPDATE items SET created_at = $2, valid_from = $3, tx_from = $4, updated_at = $5
                 WHERE id = $1",
                &[
                    &stored.id,
                    &fixed.created_at,
                    &fixed.valid_from,
                    &fixed.tx_from,
                    &fixed.updated_at,
                ],
            )
            .am("timestamp backfill write")?;
        rewritten += 1;
    }
    Ok(rewritten)
}

/// The hub a row was distilled from, when the row is a distilled unit.
///
/// A distilled unit's snapshot holds no `hub_id` of its own: the snapshot is
/// sealed inside the row's content, and the derivation record is what projects
/// it. The record's own `hub_id` is that value in plaintext, so the pass reads
/// it from there.
fn hub_column() -> String {
    "(SELECT hub_id FROM distilled_units d WHERE d.item_id = items.id)".to_owned()
}

fn stored_column<'a>(row: &'a StoredStamps, column: &str) -> &'a str {
    match column {
        "created_at" => &row.created_at,
        "valid_from" => &row.valid_from,
        "tx_from" => &row.tx_from,
        _ => &row.updated_at,
    }
}

#[cfg(test)]
#[path = "migrate_ts_backfill_tests.rs"]
mod migrate_ts_backfill_tests;
