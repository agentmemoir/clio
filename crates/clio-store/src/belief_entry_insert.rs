// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the confidence-entry INSERT bodies shared by
//! both backends' belief write transactions.
//!
//! ## Owns
//! - `insert_entry_sqlite` / `insert_entry_pg`: the append-only row insert into
//!   `belief_confidence_entries`
//!
//! ## Does not own
//! - The surrounding belief create/append transactions (`belief_tx`)
//! - Entry validation, id generation, or audit rows
//!
//! ## Boundary
//!
//! Changes to how one trajectory row is written should normally require changes
//! here. Changes to transaction flow should not require changes here.
//!
//! Keep this module focused on the entry INSERT bodies.

use crate::migrate::IntoAmError;
use clio_types::{AmError, ConfidenceEntry};

/// Insert one append-only trajectory entry on SQLite.
pub(crate) fn insert_entry_sqlite(
    tx: &rusqlite::Transaction<'_>,
    entry_id: &str,
    belief_id: &str,
    entry: &ConfidenceEntry,
    ts: &str,
) -> Result<(), AmError> {
    use rusqlite::params;
    tx.execute(
        "INSERT INTO belief_confidence_entries (
            id, belief_id, as_of, confidence, evidence_ref, source_type, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            entry_id,
            belief_id,
            entry.as_of,
            entry.confidence,
            entry.evidence_ref,
            entry.source_type.as_str(),
            ts,
        ],
    )
    .am("belief entry insert")?;
    Ok(())
}

/// Insert one append-only trajectory entry on Postgres.
pub(crate) fn insert_entry_pg(
    tx: &mut postgres::Transaction<'_>,
    entry_id: &str,
    belief_id: &str,
    entry: &ConfidenceEntry,
    ts: &str,
) -> Result<(), AmError> {
    tx.execute(
        "INSERT INTO belief_confidence_entries (
            id, belief_id, as_of, confidence, evidence_ref, source_type, created_at
         ) VALUES ($1, $2, $3, $4::float8, $5, $6, $7)",
        &[
            &entry_id,
            &belief_id,
            &entry.as_of,
            &entry.confidence,
            &entry.evidence_ref,
            &entry.source_type.as_str(),
            &ts,
        ],
    )
    .am("belief entry insert")?;
    Ok(())
}
