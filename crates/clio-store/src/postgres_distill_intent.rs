// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres storage of a wave's distill intent:
//! the claim write that takes it, the completion write, and the bank-scoped
//! read.
//!
//! ## Owns
//! - The `distill_intents` Postgres SQL and row decoding
//! - `write_distill_intent` / `complete_distill_intent` /
//!   `get_distill_intent` Postgres implementations
//!
//! ## Does not own
//! - The intent shape and validation (`distill_intent`)
//! - SQLite SQL (`sqlite_distill_intent`)
//! - The wave's ordering of the two writes (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to the Postgres intent writes should normally require changes here.
//! Changes to the intent shape should not require changes here.
//!
//! Keep this module focused on Postgres intent persistence.

use crate::distill_intent::{DistillIntent, validate_completion};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use clio_types::AmError;

/// Shared column projection for the `distill_intents` table.
const SELECT_COLS: &str = "bank_id, hub_id, unit_item_id, actor, claimed_at, revision_item_id";

/// The claim write: one row per bank and hub, and `ON CONFLICT` fires only on
/// the named primary key, so a `NOT NULL` violation is still reported rather
/// than swallowed.
const CLAIM_INTENT: &str = "INSERT INTO distill_intents (
        bank_id, hub_id, unit_item_id, actor, claimed_at, revision_item_id
     ) VALUES ($1, $2, $3, $4, $5, $6)
     ON CONFLICT (bank_id, hub_id) DO NOTHING";

/// The completion write: one conditional `UPDATE`, so a claim is completed only
/// while it is still incomplete and a finished claim is never downgraded.
/// Read Committed re-evaluates a `WHERE` clause against the concurrently updated
/// version of a row, so the predicate is evaluated where the write is attempted.
const COMPLETE_INTENT: &str = "UPDATE distill_intents SET revision_item_id = $3
     WHERE bank_id = $1 AND hub_id = $2 AND revision_item_id = ''";

fn map_row(row: &postgres::Row) -> DistillIntent {
    DistillIntent {
        bank_id: row.get(0),
        hub_id: row.get(1),
        unit_item_id: row.get(2),
        actor: row.get(3),
        claimed_at: row.get(4),
        revision_item_id: row.get(5),
    }
}

impl PostgresStore {
    /// Claim write. See [`crate::DistilledUnitStore::write_distill_intent`].
    pub(crate) fn write_distill_intent_impl(
        &self,
        bank_id: &str,
        intent: &DistillIntent,
    ) -> Result<bool, AmError> {
        intent.validate_for_bank(bank_id)?;
        self.with_client(|client| {
            let n = client
                .execute(
                    CLAIM_INTENT,
                    &[
                        &intent.bank_id,
                        &intent.hub_id,
                        &intent.unit_item_id,
                        &intent.actor,
                        &intent.claimed_at,
                        &intent.revision_item_id,
                    ],
                )
                .am("distill intent claim write pg")?;
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
        self.with_client(|client| {
            let n = client
                .execute(COMPLETE_INTENT, &[&bank_id, &hub_id, &revision_item_id])
                .am("distill intent completion write pg")?;
            Ok(n > 0)
        })
    }

    /// Bank-scoped intent read. See [`crate::DistilledUnitStore::get_distill_intent`].
    pub(crate) fn get_distill_intent_impl(
        &self,
        bank_id: &str,
        hub_id: &str,
    ) -> Result<Option<DistillIntent>, AmError> {
        self.with_client(|client| {
            let row = client
                .query_opt(
                    &format!(
                        "SELECT {SELECT_COLS} FROM distill_intents
                         WHERE bank_id = $1 AND hub_id = $2"
                    ),
                    &[&bank_id, &hub_id],
                )
                .am("distill intent get pg")?;
            Ok(row.map(|r| map_row(&r)))
        })
    }
}
