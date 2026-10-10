// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite storage of a wave's distill intent:
//! the claim write that takes it, the completion write, and the bank-scoped
//! read.
//!
//! ## Owns
//! - The `distill_intents` SQLite SQL and row decoding
//! - `write_distill_intent` / `complete_distill_intent` /
//!   `get_distill_intent` SQLite implementations
//!
//! ## Does not own
//! - The intent shape and validation (`distill_intent`)
//! - Postgres SQL (`postgres_distill_intent`)
//! - The wave's ordering of the two writes (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to the SQLite intent writes should normally require changes here.
//! Changes to the intent shape should not require changes here.
//!
//! Keep this module focused on SQLite intent persistence.

use crate::distill_intent::{DistillIntent, validate_completion};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::AmError;
use rusqlite::{OptionalExtension, params};

/// Shared column projection for the `distill_intents` table.
const SELECT_COLS: &str = "bank_id, hub_id, unit_item_id, actor, claimed_at, revision_item_id";

/// The claim write: one row per bank and hub, and `ON CONFLICT` fires only on
/// the named primary key, so a `NOT NULL` violation is still reported rather
/// than swallowed.
const CLAIM_INTENT: &str = "INSERT INTO distill_intents (
        bank_id, hub_id, unit_item_id, actor, claimed_at, revision_item_id
     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
     ON CONFLICT (bank_id, hub_id) DO NOTHING";

/// The completion write: one conditional `UPDATE`, so a claim is completed only
/// while it is still incomplete and a finished claim is never downgraded. Plain
/// equality is enough: the column is `NOT NULL`, so there is no NULL to match
/// and the null-safe `IS` adds nothing.
const COMPLETE_INTENT: &str = "UPDATE distill_intents SET revision_item_id = ?3
     WHERE bank_id = ?1 AND hub_id = ?2 AND revision_item_id = ''";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DistillIntent> {
    Ok(DistillIntent {
        bank_id: row.get(0)?,
        hub_id: row.get(1)?,
        unit_item_id: row.get(2)?,
        actor: row.get(3)?,
        claimed_at: row.get(4)?,
        revision_item_id: row.get(5)?,
    })
}

impl SqliteStore {
    /// Claim write. See [`crate::DistilledUnitStore::write_distill_intent`].
    pub(crate) fn write_distill_intent_impl(
        &self,
        bank_id: &str,
        intent: &DistillIntent,
    ) -> Result<bool, AmError> {
        intent.validate_for_bank(bank_id)?;
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    CLAIM_INTENT,
                    params![
                        intent.bank_id,
                        intent.hub_id,
                        intent.unit_item_id,
                        intent.actor,
                        intent.claimed_at,
                        intent.revision_item_id,
                    ],
                )
                .am("distill intent claim write")?;
            Ok(n > 0)
        })
    }

    /// Completion write. See [`crate::DistilledUnitStore::complete_distill_intent`].
    pub(crate) fn complete_distill_intent_impl(
        &self,
        bank_id: &str,
        hub_id: &str,
        revision_item_id: &str,
    ) -> Result<bool, AmError> {
        validate_completion(bank_id, hub_id, revision_item_id)?;
        self.with_conn(|conn| {
            let n = conn
                .execute(COMPLETE_INTENT, params![bank_id, hub_id, revision_item_id])
                .am("distill intent completion write")?;
            Ok(n > 0)
        })
    }

    /// Bank-scoped intent read. See [`crate::DistilledUnitStore::get_distill_intent`].
    pub(crate) fn get_distill_intent_impl(
        &self,
        bank_id: &str,
        hub_id: &str,
    ) -> Result<Option<DistillIntent>, AmError> {
        self.with_conn(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {SELECT_COLS} FROM distill_intents
                     WHERE bank_id = ?1 AND hub_id = ?2"
                ),
                params![bank_id, hub_id],
                map_row,
            )
            .optional()
            .am("distill intent get")
        })
    }
}
