// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite triple-trajectory reads used by
//! the history surface.
//!
//! ## Owns
//! - Triple history SELECT SQL (all-versions and point-in-time) and row mapping
//!
//! ## Does not own
//! - Task/failure reads or the `HistoryStore` trait surface
//!   (see `sqlite_history_read`)
//!
//! ## Boundary
//!
//! Changes to SQLite triple-history SQL should normally require changes here.
//! Changes to task/failure history reads should not.

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::triple_map::map_triple_row;
use clio_types::{AmError, TimeAxis, TripleRecord};
use rusqlite::OptionalExtension;
use rusqlite::params;

pub(crate) const TRIPLE_COLS: &str = "id, bank_id, item_id, subject, predicate, object_ciphertext, \
     epistemic_kind, source_type, confidence, valid_from, valid_until, tx_from, tx_until, \
     snapshot_ref, gist_ref, created_at, updated_at";

impl SqliteStore {
    /// Attach the carrier item's sealed descriptive `context` to each record.
    ///
    /// `None` covers an absent body, no stored context, or an unreadable body
    /// (for example a crypto-shredded subject) so the history read stays
    /// available after erasure.
    fn attach_carrier_context(&self, records: &mut [TripleRecord]) -> Result<(), AmError> {
        for record in records {
            let cipher: Option<String> = self
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT content_ciphertext FROM items WHERE id = ?1",
                        params![record.item_id],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()
                    .am("triple history carrier context")
                })?
                .flatten();
            record.context =
                crate::item_persist::open_context(self.kms().as_ref(), cipher.as_deref());
        }
        Ok(())
    }

    /// Open each history row's sealed asserted value and build the
    /// record. The trajectory query is keyed on `subject` + `predicate`, which
    /// stay plaintext, so only the value needs the DEK.
    fn record_from_row(&self, t: TripleRowValues) -> Result<TripleRecord, AmError> {
        map_triple_row(
            t.0,
            t.1,
            t.2,
            t.3,
            t.4,
            crate::triple_cipher::open_optional_object(self.kms().as_ref(), t.5.as_deref())?,
            t.6,
            t.7,
            t.8,
            t.9,
            t.10,
            t.11,
            t.12,
            t.13,
            t.14,
            t.15,
            t.16,
        )
    }

    pub(crate) fn triple_history_impl(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {TRIPLE_COLS} FROM triples
                     WHERE bank_id = ?1 AND subject = ?2 AND predicate = ?3
                     ORDER BY tx_from, id LIMIT ?4"
                ))
                .am("triple history prepare")?;
            let rows = stmt
                .query_map(
                    params![bank_id, subject, predicate, limit as i64],
                    triple_row,
                )
                .am("triple history query")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(self.record_from_row(row.am("triple history row")?)?);
            }
            Ok(out)
        })
        .and_then(|mut out| {
            self.attach_carrier_context(&mut out)?;
            Ok(out)
        })
    }

    pub(crate) fn triple_history_at_impl(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        as_of: &str,
        axis: TimeAxis,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError> {
        let axis_col = match axis {
            TimeAxis::Valid => "valid_from",
            TimeAxis::Transaction => "tx_from",
        };
        self.with_conn(|conn| {
            // Newest `limit` versions at/under `as_of`, then oldest-first.
            // The axis predicate runs in the inner query so LIMIT cannot drop
            // the version current at `as_of`.
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {TRIPLE_COLS} FROM (
                         SELECT {TRIPLE_COLS} FROM triples
                         WHERE bank_id = ?1 AND subject = ?2 AND predicate = ?3
                           AND {axis_col} <= ?4
                         ORDER BY {axis_col} DESC, id DESC LIMIT ?5
                     ) ORDER BY {axis_col} ASC, id ASC"
                ))
                .am("triple history at prepare")?;
            let rows = stmt
                .query_map(
                    params![bank_id, subject, predicate, as_of, limit as i64],
                    triple_row,
                )
                .am("triple history at query")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(self.record_from_row(row.am("triple history at row")?)?);
            }
            Ok(out)
        })
        .and_then(|mut out| {
            self.attach_carrier_context(&mut out)?;
            Ok(out)
        })
    }
}

/// One `TRIPLE_COLS` row. Index 5 is the sealed asserted value, so it is
/// `Option<String>`: a migrated row always has one, but a NULL body must map to
/// a named unreadable-content error rather than to an empty value.
#[allow(clippy::type_complexity)]
type TripleRowValues = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<f64>,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

fn triple_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TripleRowValues> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
        r.get(15)?,
        r.get(16)?,
    ))
}
