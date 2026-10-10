// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hub-distill wave's claim ordering: writing the distill intent before one hub's work starts, completing it with the produced revision after the unit lands, and noting a resumed claim on the outcome.
//!
//! ## Owns
//! - `claim_before_work`, the claim write plus whether this wave resumed one
//! - `complete_claim_with_revision`, the completion write that finishes a claim
//! - `annotate_resumed`, the resume note on an outcome
//!
//! ## Does not own
//! - When a wave claims or resumes (`hub_distill`)
//! - The intent shape and its SQL (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to what a wave writes down about its own work should normally require changes here.
//! Changes to what the work itself is should not require changes here.
//!
//! Keep this module focused on the claim write, its completion, and its note.

#[cfg(test)]
#[path = "hub_distill_claim_tests.rs"]
mod hub_distill_claim_tests;

use crate::hub_distill_model::DistillOutcome;
use clio_store::{DistillIntent, Store};
use clio_types::AmError;

/// Write this wave's claim for one hub, and say whether this wave resumes one.
///
/// The write never overwrites: the first claim's actor, unit id, and time stand, so a second wave's claim for the same pair changes nothing. The return is true only when the bank already held an INCOMPLETE claim for this hub, so this wave's write was a no-op and the wave is resuming. A claim this wave just wrote, and a claim an earlier wave completed, both read as not resumed: the first is new work, the second is done work, and neither changes what the wave does next.
pub(crate) fn claim_before_work(
    store: &dyn Store,
    bank: &str,
    hub_id: &str,
    unit_item_id: &str,
    actor: &str,
    claimed_at: &str,
) -> Result<bool, AmError> {
    let intent = DistillIntent::new(bank, hub_id, unit_item_id, actor, claimed_at);
    let inserted = store.write_distill_intent(bank, &intent)?;
    if inserted {
        return Ok(false);
    }
    Ok(store
        .get_distill_intent(bank, hub_id)?
        .is_some_and(|existing| !existing.is_complete()))
}

/// Record the revision one hub's fresh distillation produced, as the last step of that success.
///
/// Refusals and errors never reach here: their claims stay incomplete so the next wave re-evaluates them exactly as today.
pub(crate) fn complete_claim_with_revision(
    store: &dyn Store,
    bank: &str,
    hub_id: &str,
    revision_item_id: &str,
) -> Result<bool, AmError> {
    store.complete_distill_intent(bank, hub_id, revision_item_id)
}

/// Note a resumed claim on an outcome the fresh or reconcile path produced.
///
/// Report lists ignore `reason`, so this changes no list and no status: the note survives on the returned outcome only. An outcome that already carries a determination reason keeps it, so the resume never erases why a wave refused or kept a unit stale.
pub(crate) fn annotate_resumed(outcome: &mut DistillOutcome, resumed: bool) {
    if resumed && outcome.reason.is_none() {
        outcome.reason =
            Some("resumed an incomplete distill claim from an earlier wave".to_owned());
    }
}
