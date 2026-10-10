// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite side of the distilled-unit store
//! contract: the `DistilledUnitStore` delegation for `SqliteStore` and the
//! compare-and-set claim/stale write a caller makes from a record it read
//! earlier.
//!
//! ## Owns
//! - The `impl DistilledUnitStore for SqliteStore` delegation surface
//! - `SqliteStore::update_distilled_unit_if_unchanged_impl` and the conditional
//!   `UPDATE` whose `WHERE` clause carries the state the caller expected
//!
//! ## Does not own
//! - The record shape and validation (`distilled_unit`) or the contract's
//!   documented behaviour (`distilled_unit_store`)
//! - The other `distilled_units` SQL: insert, read, the revision write, the
//!   unconditional claim/stale update, and the discard-path cascade
//!   (`sqlite_distilled_unit`)
//! - Postgres SQL (`postgres_distilled_unit_store`)
//!
//! ## Boundary
//!
//! Changes to the SQLite distilled-unit contract surface, or to the guard that
//! decides whether a caller's earlier read is still the stored state, should
//! normally require changes here. Changes to the record shape or to the
//! unconditional write should not require changes here.
//!
//! Keep this module focused on the SQLite persistence contract for derived
//! units.

use crate::distill_intent::DistillIntent;
use crate::distilled_evidence::encode_evidence;
use crate::distilled_unit::{DistilledUnitRecord, encode_source_ids};
use crate::item_persist::require_actor;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::sqlite_distilled_unit::reproject_snapshot_tx;
use clio_types::{AmError, ErrorCode, MemoryItem};
use rusqlite::{Transaction, params};

impl SqliteStore {
    /// Compare-and-set claim/stale update + snapshot re-projection. See
    /// [`crate::distilled_unit_store::DistilledUnitStore::update_distilled_unit_if_unchanged`].
    pub(crate) fn update_distilled_unit_if_unchanged_impl(
        &self,
        expected: &DistilledUnitRecord,
        updated: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<bool, AmError> {
        updated.validate()?;
        require_same_unit(expected, updated)?;
        require_actor(actor)?;
        self.with_conn(|conn| {
            let tx = conn
                .transaction()
                .am("distilled unit compare-and-set begin")?;
            if !reproject_snapshot_tx(&tx, self.kms().as_ref(), updated)? {
                // A dead unit: nothing is written, exactly as the unconditional
                // update leaves it, and nothing is reported as a lost race.
                return Ok(false);
            }
            if !update_record_if_unchanged_tx(&tx, expected, updated)? {
                // Nothing was applied, so the re-projection above must not land
                // either: dropping the transaction without committing rolls both
                // back, and the row keeps the state it actually has.
                return Ok(false);
            }
            tx.commit().am("distilled unit compare-and-set commit")?;
            Ok(true)
        })
    }
}

/// Conditional record update: the write lands only while the stored row is still
/// `expected` on every column this write would replace.
///
/// `Ok(false)` means the row is not in the expected state, or is gone: the
/// caller read a unit that has since moved, so its write is refused. The
/// comparison is null-safe (`IS`, not `=`) because `stale_reason`,
/// `stale_detail` and `stale_at` are nullable, and `=` would report every
/// current unit as moved.
fn update_record_if_unchanged_tx(
    tx: &Transaction<'_>,
    expected: &DistilledUnitRecord,
    updated: &DistilledUnitRecord,
) -> Result<bool, AmError> {
    let n = tx
        .execute(
            "UPDATE distilled_units SET
                source_ids_json = ?1, marker = ?2, record_version = ?3, distilled_at = ?4,
                revision = ?5, stale_reason = ?6, stale_detail = ?7, stale_at = ?8,
                evidence_json = ?9
             WHERE bank_id = ?10 AND item_id = ?11
               AND source_ids_json IS ?12 AND marker IS ?13 AND record_version IS ?14
               AND distilled_at IS ?15 AND revision IS ?16 AND stale_reason IS ?17
               AND stale_detail IS ?18 AND stale_at IS ?19 AND evidence_json IS ?20",
            params![
                encode_source_ids(&updated.source_ids),
                updated.marker,
                i64::from(updated.record_version),
                updated.distilled_at,
                i64::from(updated.revision),
                updated.stale_reason,
                updated.stale_detail,
                updated.stale_at,
                encode_evidence(&updated.evidence),
                updated.bank_id,
                updated.item_id,
                encode_source_ids(&expected.source_ids),
                expected.marker,
                i64::from(expected.record_version),
                expected.distilled_at,
                i64::from(expected.revision),
                expected.stale_reason,
                expected.stale_detail,
                expected.stale_at,
                encode_evidence(&expected.evidence),
            ],
        )
        .am("distilled unit record compare-and-set")?;
    Ok(n > 0)
}

/// The two records must describe the same row, or the predicate would compare a
/// row the caller never read.
fn require_same_unit(
    expected: &DistilledUnitRecord,
    updated: &DistilledUnitRecord,
) -> Result<(), AmError> {
    if expected.bank_id != updated.bank_id || expected.item_id != updated.item_id {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit compare-and-set needs both records for the same unit",
        ));
    }
    Ok(())
}

impl crate::distilled_unit_store::DistilledUnitStore for SqliteStore {
    fn list_distilled_units(&self, bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError> {
        self.list_distilled_units_impl(bank_id)
    }

    fn revise_distilled_unit(
        &self,
        record: &DistilledUnitRecord,
        new_item: &MemoryItem,
        actor: &str,
    ) -> Result<(), AmError> {
        self.revise_distilled_unit_impl(record, new_item, actor)
    }

    fn update_distilled_unit(
        &self,
        record: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<(), AmError> {
        self.update_distilled_unit_impl(record, actor)
    }

    fn update_distilled_unit_if_unchanged(
        &self,
        expected: &DistilledUnitRecord,
        updated: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<bool, AmError> {
        self.update_distilled_unit_if_unchanged_impl(expected, updated, actor)
    }

    fn distilled_unit_claim_state(
        &self,
        bank_id: &str,
        claim_ids: &[String],
    ) -> Result<Vec<crate::distilled_freshness::ClaimedSourceState>, AmError> {
        self.distilled_unit_claim_state_impl(bank_id, claim_ids)
    }

    fn write_distill_intent(&self, bank_id: &str, intent: &DistillIntent) -> Result<bool, AmError> {
        self.write_distill_intent_impl(bank_id, intent)
    }

    fn complete_distill_intent(
        &self,
        bank_id: &str,
        hub_id: &str,
        revision_item_id: &str,
    ) -> Result<bool, AmError> {
        self.complete_distill_intent_impl(bank_id, hub_id, revision_item_id)
    }

    fn get_distill_intent(
        &self,
        bank_id: &str,
        hub_id: &str,
    ) -> Result<Option<DistillIntent>, AmError> {
        self.get_distill_intent_impl(bank_id, hub_id)
    }
}
