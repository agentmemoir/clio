// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres operations-removal cascade over
//! hub-distilled derivation records: dropping a discarded item id from every
//! claim set in the bank, marking the affected units stale, re-projecting their
//! current snapshot, and writing the audit rows.
//!
//! ## Owns
//! - The cascade loop over one bank's records (`cascade_source_removed_tx`)
//! - The audit id disambiguator the cascade needs to stay unique per bank
//!
//! ## Does not own
//! - Record SQL and row decoding (see `postgres_distilled_unit`)
//! - Snapshot re-projection (see `postgres_distilled_unit`)
//! - Discard validation and telemetry (`postgres_discard`)
//!
//! ## Boundary
//!
//! Changes to what the cascade writes should normally require changes here.
//! Changes to record shape or read SQL should not require changes here.
//!
//! Keep this module focused on the Postgres discard-path cascade.

use crate::migrate::IntoAmError;
use crate::postgres_distilled_unit::{list_tx, reproject_snapshot_tx, update_record_tx};
use clio_types::AmError;
use std::sync::atomic::{AtomicU64, Ordering};

/// Audit-id disambiguator: two cascade events in one millisecond must not
/// collide on the `audit_events.id` primary key.
static CASCADE_AUDIT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Operations-removal cascade, inside the discard transaction: every unit in
/// the bank whose claim set contains the removed item drops that id from its
/// claim set and is marked stale with reason `source_removed`, and its current
/// revision item's snapshot is re-projected from the same record state.
///
/// A unit whose current content is unreadable for erasure reasons is left
/// untouched (dead unit; erasure evidence). A discarded unit id that is the
/// record's hub leaves the source ids and the current evidence alone: the hub
/// column is the unit's identity, the unit stays stale, and reconciliation of a
/// discarded hub never rebuilds. Telemetry is ids and counts only.
pub(crate) fn cascade_source_removed_tx(
    tx: &mut postgres::Transaction<'_>,
    kms: &dyn crate::dek::DekProvider,
    bank_id: &str,
    removed_id: &str,
    discarded_at: &str,
    actor: &str,
) -> Result<(), AmError> {
    for mut record in list_tx(&mut *tx, bank_id)? {
        if !record.claim_ids().contains(removed_id) {
            continue;
        }
        let claim_before = record.claim_ids().len();
        record.source_ids.retain(|id| id != removed_id);
        // The current revision's evidence list follows the claim set: a source
        // that left active use stops counting toward the proof count. Earlier
        // revisions keep their entries as history of what was claimed then. The
        // hub is the exception: `claim_ids()` re-inserts it as the unit's
        // identity, so a discarded hub stays a claim and keeps its entry too.
        if removed_id != record.hub_id {
            record.drop_source_from_current_evidence(removed_id);
        }
        record.stale_reason = Some(crate::distilled_unit::STALE_SOURCE_REMOVED.to_owned());
        record.stale_at = Some(discarded_at.to_owned());
        record.stale_detail = None;
        if !reproject_snapshot_tx(tx, kms, &record)? {
            continue;
        }
        update_record_tx(tx, &record)?;
        let seq = CASCADE_AUDIT_SEQ.fetch_add(1, Ordering::Relaxed);
        tx.execute(
            "INSERT INTO audit_events (
                id, bank_id, operation, item_id, category, epistemic_kind,
                confidence, admission_score, actor, detail_json, created_at
             ) VALUES ($1, $2, 'hub_distill_source_removed', $3, NULL, NULL, NULL, NULL, $4, $5, $6)",
            &[
                &format!("audit-hubdistill-cascade-{}-{discarded_at}-{seq}", record.item_id),
                &bank_id,
                &record.item_id,
                &actor,
                &serde_json::json!({
                    "removed_id": removed_id,
                    "claim_before": claim_before,
                    "claim_after": record.claim_ids().len(),
                })
                .to_string(),
                &discarded_at,
            ],
        )
        .am("distilled cascade audit insert pg")?;
    }
    Ok(())
}
