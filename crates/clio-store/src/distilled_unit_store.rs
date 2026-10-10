// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the distilled-unit store contract: the
//! record list, the revision write, and the claim/stale update that
//! reconciliation and the discard cascade use. A supertrait of [`Store`], so a
//! `dyn Store` exposes it and the contract lives beside the record shape.
//!
//! ## Owns
//! - `DistilledUnitStore`
//!
//! ## Does not own
//! - The record shape and validation (`distilled_unit`)
//! - Backend SQL (`sqlite_distilled_unit` / `postgres_distilled_unit`)
//! - The rest of the store contract (`store`)
//!
//! ## Boundary
//!
//! Changes to the distilled-unit store contract should normally require
//! changes here. Changes to other store contracts should not require changes
//! here.
//!
//! Keep this module focused on the distilled-unit store contract.

use crate::distill_intent::DistillIntent;
use crate::distilled_freshness::ClaimedSourceState;
use crate::distilled_unit::DistilledUnitRecord;
use clio_types::{AmError, MemoryItem};

/// The distilled-unit persistence contract.
pub trait DistilledUnitStore: Send + Sync {
    /// Every distilled unit's derivation record in a bank, ordered by unit id
    /// (deterministic); the reconciliation pass reads this once per wave.
    /// # Errors: storage failures.
    fn list_distilled_units(&self, bank_id: &str) -> Result<Vec<DistilledUnitRecord>, AmError>;

    /// Write one revision of a distilled unit in one transaction: close the
    /// prior revision on both axes (retained, lineage-linked), insert the new
    /// revision item, and update the record — keyed by the stable unit id — so
    /// item and claim-set state land together. Item-keyed lineage; never
    /// resolved through `(bank, subject)`.
    /// # Errors: validation/mismatch, a closed or missing prior revision, or storage.
    fn revise_distilled_unit(
        &self,
        record: &DistilledUnitRecord,
        new_item: &MemoryItem,
        actor: &str,
    ) -> Result<(), AmError>;

    /// Persist one distilled unit's updated derivation record (claim set,
    /// reconciliation state, and/or evidence list) and re-project the current
    /// revision item's snapshot from the same record state, in one transaction,
    /// so record and snapshot cannot drift. A unit whose current content is
    /// unreadable (erased or shredded) is left untouched: a dead unit's record is
    /// erasure evidence.
    /// # Errors: validation/mismatch or storage.
    fn update_distilled_unit(
        &self,
        record: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<(), AmError>;

    /// [`Self::update_distilled_unit`] under a compare-and-set: persist `updated`
    /// only while the stored row is still `expected`, and re-project the current
    /// revision item's snapshot from the same record state in the same
    /// transaction, exactly as that method does.
    ///
    /// `expected` is the record as the caller last read it. The write lands only
    /// when the row still carries that claim set, reconciliation stamp, revision,
    /// stale state and evidence list, and `Ok(false)` says it does not: someone
    /// else moved the unit while the caller was deciding, and the decision was
    /// made against a state that no longer exists. The operations-removal
    /// cascade is that someone — it marks a unit and drops the removed id from
    /// its claim set inside the discard's own transaction — so the cascade wins
    /// the race instead of being undone by a wave's or a refusal's write.
    ///
    /// One conditional `UPDATE` per backend, so the predicate is evaluated
    /// against the row as it stands when the write is attempted: Postgres
    /// re-evaluates a `WHERE` clause against the concurrently updated version of
    /// a row, and SQLite holds the single write lock across the statement.
    ///
    /// This is the write for a caller that judged the unit from a record it read
    /// earlier. `update_distilled_unit` stays the general persister for a caller
    /// that means to change the claim set itself.
    ///
    /// # Errors: validation/mismatch or storage. A dead unit is left untouched,
    /// as in [`Self::update_distilled_unit`], and reports `Ok(false)` for a
    /// reason that is not a lost race.
    fn update_distilled_unit_if_unchanged(
        &self,
        expected: &DistilledUnitRecord,
        updated: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<bool, AmError>;

    /// Read the metadata state of the given claimed sources: each id's
    /// last-change time (absent when the row is gone) and whether it is still in
    /// active use.
    ///
    /// This is the only read freshness needs from a source: it answers "is it
    /// still here" and "when did it last change" without decrypting a payload,
    /// so a `get` on a unit does not open every item it summarizes. Ids the
    /// caller asks for that the bank does not hold still come back, marked
    /// inactive, so a caller never has to distinguish "absent" from "not
    /// reported".
    /// # Errors: storage failures.
    fn distilled_unit_claim_state(
        &self,
        bank_id: &str,
        claim_ids: &[String],
    ) -> Result<Vec<ClaimedSourceState>, AmError>;

    /// Write down what a wave is about to do, before it does it: one row per
    /// bank and hub, carrying the deterministic unit id, the wave's actor, the
    /// claim time, and an empty completion state.
    ///
    /// `Ok(true)` says this call took the claim. `Ok(false)` says the bank
    /// already holds one for that hub and nothing was changed, so a claim is
    /// never overwritten by a weaker one: the first actor, unit id, and claim
    /// time are the record. The row holds ids and timestamps only, so a claim
    /// carries no content.
    ///
    /// A claim is written once and completed rather than downgraded, and it is never deleted on its own: the per-item erasure path removes it, in the same transaction, with the hub or the distilled unit it names, so an interrupted wave still leaves a claim a later run can recognise.
    ///
    /// A claim is only ever finished by [`Self::complete_distill_intent`], and
    /// the claim write refuses an intent that already carries a revision id, so
    /// a claim can never be stored finished.
    /// # Errors: validation, including a pre-completed intent, a bank that is not the intent's own, or storage.
    fn write_distill_intent(&self, bank_id: &str, intent: &DistillIntent) -> Result<bool, AmError>;

    /// Record the revision a claim produced. One conditional write, so it lands
    /// only while the claim is still incomplete.
    ///
    /// `Ok(true)` says this call finished the claim. `Ok(false)` says the claim
    /// was already complete, or that the bank holds none for that hub; either
    /// way the stored record is left exactly as it was, and a complete claim is
    /// never downgraded to a weaker state.
    /// # Errors: an empty bank, hub, or revision id, or storage.
    fn complete_distill_intent(
        &self,
        bank_id: &str,
        hub_id: &str,
        revision_item_id: &str,
    ) -> Result<bool, AmError>;

    /// Read one bank's claim for a hub, so an interrupted wave can be
    /// recognised, or `Ok(None)` when the bank holds none. Bank-scoped: another
    /// bank's claim is never returned.
    /// # Errors: storage failures.
    fn get_distill_intent(
        &self,
        bank_id: &str,
        hub_id: &str,
    ) -> Result<Option<DistillIntent>, AmError>;
}
