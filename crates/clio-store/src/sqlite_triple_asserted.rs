// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one question on the SQLite triple store: whether
//! a subject, predicate, and optional object were ever asserted in a bank, open or
//! ended.
//!
//! ## Owns
//! - The ever-asserted lookup behind the `triple_asserted_ever` store method
//!
//! ## Does not own
//! - Closing triples (see `sqlite_triple_read`)
//! - The sealed object format and its comparison (see `triple_cipher`)
//! - The decision to refuse an unknown triple (see the write layer)
//!
//! ## Boundary
//!
//! Changes to how a triple is closed should not require changes here. Changes to
//! the question "was it ever asserted" should normally require changes here only.
//!
//! Keep this module focused on the ever-asserted lookup.

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::triple_cipher::object_matches;
use clio_types::AmError;
use rusqlite::params;

impl SqliteStore {
    /// Whether any row for this subject and predicate, open or ended, matches the
    /// object. With no object given, any row for the pair counts.
    pub(crate) fn triple_asserted_ever_impl(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        object: Option<&str>,
    ) -> Result<bool, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT object_ciphertext FROM triples
                     WHERE bank_id = ?1 AND subject = ?2 AND predicate = ?3",
                )
                .am("triple asserted prepare")?;
            let rows = stmt
                .query_map(params![bank_id, subject, predicate], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .am("triple asserted query")?;
            for row in rows {
                let cipher = row.am("triple asserted row")?;
                if object_matches(self.kms().as_ref(), cipher.as_deref(), object)? {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }
}

#[cfg(test)]
#[path = "sqlite_triple_asserted_tests.rs"]
mod tests;
