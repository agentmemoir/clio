// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reconciliation of existing distilled
//! units inside a `hub_distill` wave: the change comparison against the
//! derivation record, the revision write that re-enters admission, the stale
//! marking on a refused revision, and the bounded unit-enumeration pass.
//!
//! ## Owns
//! - `HubDistillEngine::reconcile_unit`, the per-unit reconcile decision, and the
//!   per-revision evidence list a successful revision records
//! - `HubDistillEngine::reconcile_units_pass`, the wave's enumeration pass, and
//!   the landed unit's distill-claim completion the pass finishes
//! - The reconciliation audit rows (`hub_distill_reconcile`)
//!
//! ## Does not own
//! - First distillation and the wave envelope (`hub_distill`)
//! - The claim write and its lifecycle (`hub_distill_claim`)
//! - Scans: eligibility and neighbor selection (`hub_distill_scan`)
//! - Shapes and pure builders (`hub_distill_model`)
//! - The revision store primitive and the discard cascade (`clio-store`)
//! - Recall's stale read rule (`clio-retrieve`)
//!
//! ## Boundary
//!
//! Changes to when a known unit is re-derived or marked stale should normally
//! require changes here. Changes to the revision SQL or the stale read rule
//! should not require changes here.
//!
//! Keep this module focused on reconciling known distilled units.

use super::HubDistillEngine;
use crate::hub_distill_claim::complete_claim_with_revision;
use crate::hub_distill_model::{
    DistillOutcome, HubSignalSource, build_distill_item, claimed_evidence, modal_category,
    neighbor_text,
};
use clio_admission::decision::Candidate;
use clio_admission::gated::{GateRequest, gated_create};
use clio_admission::policy::AdmissionPolicy;
use clio_config::retention::ResolvedRetention;
use clio_store::{AssocPolicy, DistilledUnitRecord, STALE_REVISION_REJECTED};
use clio_types::{AmError, ErrorCode, MemoryItem, OpContext};

use hub_distill_reconcile_outcomes::{
    failed_outcome, no_neighbors_outcome, purged_outcome, stale_hub_outcome, superseded_outcome,
    unchanged_outcome,
};

impl HubDistillEngine {
    /// Reconcile every known unit the eligible loop did not visit: hubs that
    /// fell below the degree limit, stale units, and hubs gone from active
    /// use. Bounded by the number of existing units, one pass per wave, in the
    /// record list's unit-id order.
    ///
    /// A per-unit write conflict is recorded as that unit's outcome and the
    /// pass continues. Storage reads already fail the wave, but a *write* that
    /// cannot land because its revision id already names some other row is a
    /// property of that one unit. Letting it propagate would stop
    /// reconciliation for every other unit in the bank and keep stopping it on
    /// every later wave, so one un-writable unit could freeze the bank
    /// indefinitely. The unit stays exactly as it was. Every other error,
    /// including any other storage failure, still fails the wave.
    ///
    /// A unit this pass reconciles successfully is a unit an earlier wave's
    /// work landed, so the pass finishes that wave's claim with the stored
    /// record's revision, exactly as the eligible loop's already-distilled
    /// branch does. The completion write only lands while the claim is still
    /// incomplete, so a complete claim, or a bank holding no claim for the
    /// hub, is a no-op. A failed reconcile leaves the claim incomplete for the
    /// next wave.
    ///
    /// # Errors
    /// Storage failures on the enumeration or on a unit's write, except the
    /// revision-id conflict reported as an outcome; per-unit outcomes are never
    /// silently skipped.
    pub(super) fn reconcile_units_pass(
        &self,
        bank: &str,
        ctx: &OpContext,
        retention: &ResolvedRetention,
        policy: &AssocPolicy,
        eligible: &[String],
        report: &mut crate::hub_distill_model::HubDistillReport,
    ) -> Result<(), AmError> {
        for record in self.store.list_distilled_units(bank)? {
            if eligible.contains(&record.hub_id) {
                // The eligible loop reconciled this unit already.
                continue;
            }
            let outcome =
                match self.reconcile_unit(bank, record.clone(), ctx, retention.clone(), policy) {
                    Ok(outcome) => {
                        // The unit is here and this pass just reconciled it, so
                        // an earlier wave's work for its hub landed. Finish
                        // that wave's claim with the stored record's revision:
                        // the completion write only lands while the claim is
                        // still incomplete, so a complete claim's no-write
                        // no-op here is untouched, and a bank holding no claim
                        // for the hub is one too. It runs after the reconcile,
                        // so a failed reconcile still leaves the claim
                        // incomplete for the next wave.
                        complete_claim_with_revision(
                            self.store.as_ref(),
                            bank,
                            &record.hub_id,
                            &record.current_revision_item_id(),
                        )?;
                        outcome
                    }
                    Err(err) if self.revision_id_conflict(bank, &record, &err) => {
                        failed_outcome(&record, &err.message)
                    }
                    Err(err) => return Err(err),
                };
            report.push_outcome(outcome);
        }
        Ok(())
    }

    /// Whether a failed reconcile write was this unit's next revision id landing
    /// on a row that already exists and is not one of this unit's own retained
    /// revisions.
    ///
    /// Revision ids are `{unit_id}:v{n}` and item ids are caller-controlled, so
    /// a row written by someone else can occupy a revision's id. The test is a
    /// positive lookup of that id, not a match on the error text: the store
    /// reports a duplicate item id by code and the same code can mean other
    /// things, and a lookup failure of its own is not a conflict, so a backend
    /// that is genuinely down still fails the wave.
    fn revision_id_conflict(
        &self,
        bank: &str,
        record: &DistilledUnitRecord,
        err: &AmError,
    ) -> bool {
        if err.code != ErrorCode::InvalidArgument {
            return false;
        }
        let next = clio_store::revision_item_id(&record.item_id, record.revision + 1);
        // `Err` is a conflict check that could not run, never a conflict.
        matches!(self.store.get_memory_item(bank, &next), Ok(Some(_)))
    }

    /// Reconcile one known distilled unit against its current cluster.
    ///
    /// The comparison reads the same selection policy first distillation uses:
    /// the recomputed neighbor set (discarded sources excluded) is unioned with
    /// the record's claim set — the first distillation prunes a cluster's
    /// edges, so a smaller recomputed set means nothing changed, while a new
    /// neighbor grows the claim set — and the candidate text (current hub gist
    /// plus claim-set gists) is compared with the unit's current text. A unit
    /// whose claim set is settled always rebuilds from it, and the cascade keeps
    /// it free of removed sources.
    ///
    /// # Errors
    /// Storage failures. Admission reject is `Ok`.
    pub(super) fn reconcile_unit(
        &self,
        bank: &str,
        record: DistilledUnitRecord,
        ctx: &OpContext,
        retention: ResolvedRetention,
        policy: &AssocPolicy,
    ) -> Result<DistillOutcome, AmError> {
        let current = match self
            .store
            .get_memory_item(bank, &record.current_revision_item_id())
        {
            Ok(Some(item)) => item,
            Ok(None) => return Ok(purged_outcome(&record)),
            Err(err) if erasure_dead(&err) => return Ok(purged_outcome(&record)),
            Err(err) => return Err(err),
        };
        if current.tx_until.is_some() {
            // The unit's tracked current revision was superseded outside
            // reconciliation (an operator correction): the record no longer
            // tracks the unit's text, so reconciliation leaves it alone.
            return Ok(superseded_outcome(&record));
        }
        let hub = match self.store.get_memory_item(bank, &record.hub_id) {
            Ok(Some(item)) => item,
            Ok(None) => return Ok(stale_hub_outcome(&record)),
            Err(err) if erasure_dead(&err) => return Ok(purged_outcome(&record)),
            Err(err) => return Err(err),
        };
        // A hub that left active use is a removal the cascade already marked:
        // the unit stays stale and readable, and is never rebuilt here.
        if self
            .store
            .get_item_identity(bank, &record.hub_id)?
            .is_none()
        {
            return Ok(stale_hub_outcome(&record));
        }
        let claim_sources = self.claim_source_items(bank, &record, policy)?;
        if claim_sources.is_empty() {
            // Every claimed source left active use. A unit with no sources has
            // no cluster to summarize, so there is nothing to re-derive: an
            // empty rebuild would read "hub_distill: <hub> related to " and
            // re-admit a summary with no evidence behind it. The unit keeps its
            // stale reason and stays readable, exactly as the report model
            // documents for this outcome.
            return Ok(no_neighbors_outcome(&record));
        }
        let ts = self.now_canonical();
        let mut updated = record.clone();
        updated.source_ids = claim_sources.iter().map(|i| i.id.clone()).collect();
        updated.distilled_at.clone_from(&ts);
        updated.revision += 1;
        updated.stale_reason = None;
        updated.stale_detail = None;
        updated.stale_at = None;
        // This revision's evidence is the claim set the rebuild just derived,
        // from the very items the claim ids and the candidate text come from, so
        // the stored revision, its snapshot and its evidence are one state. The
        // unchanged wave returns below before any write, so it appends nothing.
        updated.record_evidence(claimed_evidence(&hub, &claim_sources), &ts);
        let category = modal_category(&hub, &claim_sources);
        let candidate = build_distill_item(bank, &hub, category, &updated, &claim_sources, &ts);
        // The unchanged shortcut is for a unit whose record is not asking to be
        // rebuilt. A settled claim set always asks, whatever the comparison says:
        // a removal or a refusal has to be re-derived before it can clear. An
        // edit does not settle anything, so such a unit is compared like a current
        // one and a wave that finds its text and claim set unmoved writes nothing.
        if !record.claim_set_settled()
            && !reconcile_changed(&record, &updated, &current, &candidate)
        {
            return Ok(unchanged_outcome(&record));
        }
        self.revise_unit(
            bank,
            record,
            &current,
            &updated,
            &candidate,
            &claim_sources,
            category,
            ctx,
            retention,
            &ts,
        )
    }

    /// Admit and write the re-derived revision; on reject, mark the unit stale
    /// with the rejection detail and keep the prior revision current.
    #[allow(clippy::too_many_arguments)]
    fn revise_unit(
        &self,
        bank: &str,
        record: DistilledUnitRecord,
        current: &MemoryItem,
        updated: &DistilledUnitRecord,
        candidate: &MemoryItem,
        claim_sources: &[MemoryItem],
        category: clio_types::SemanticCategory,
        ctx: &OpContext,
        retention: ResolvedRetention,
        ts: &str,
    ) -> Result<DistillOutcome, AmError> {
        let claim_counts = (record.claim_ids().len(), updated.claim_ids().len());
        let admission = AdmissionPolicy::with_retention(&self.env, retention);
        let source = HubSignalSource {
            texts: claim_sources.iter().map(neighbor_text).collect(),
        };
        let mut write = |admitted: &MemoryItem| {
            self.store
                .revise_distilled_unit(updated, admitted, ctx.actor.as_str())
        };
        let decision = gated_create(
            &GateRequest {
                item: candidate,
                ctx,
                policy: admission,
                sink: self.sink.as_ref(),
                timestamp: ts,
            },
            Candidate::Semantic(category),
            &source,
            &mut write,
        )?;
        // A unit that is already stale for this refusal, with an unchanged
        // claim set and unchanged text, is a wave that found nothing: it must
        // write no audit row and re-seal no content. A first refusal is not
        // that case — it is a determination and is audited, below.
        //
        // The refusal *detail* deliberately does not take part in this
        // comparison. The detail embeds the admission score, which is not a
        // stable function of the input — the same unchanged cluster scores
        // differently across waves — so comparing it would make every retry
        // look like a change and defeat the point. What matters is that the
        // unit is already marked stale for this same reason and nothing it
        // summarizes moved. A policy change that now admits the candidate
        // still revises it, because it no longer reaches this branch.
        let already_refused = record.stale_reason.as_deref() == Some(STALE_REVISION_REJECTED)
            && !reconcile_changed(&record, updated, current, candidate);
        if !decision.pass {
            let outcome = DistillOutcome {
                hub_id: record.hub_id.clone(),
                item_id: record.current_revision_item_id(),
                status: "revision_rejected".into(),
                admission_score: Some(decision.admission_score),
                reason: decision.rejection_reason,
            };
            if already_refused {
                return Ok(outcome);
            }
            // A refusal is a determination like any other and carries its own
            // per-item derived audit, so a permanently refused unit leaves a
            // durable trace of why it was refused. Without it the engine's
            // default sink, which drops the gate's own telemetry, would make
            // the refusal invisible after the fact.
            self.audit_reconcile(bank, &record, candidate, false, &claim_counts, ctx)?;
            // A refused revision replaces nothing: the prior stays current in
            // the store, and the unit is marked stale (returned, not withheld).
            // The marking is a compare-and-set against the record this unit was
            // read as, so a discard cascade landing mid-wave keeps its own
            // determination: a refusal computed from a claim set that no longer
            // exists must not put the withdrawn source back.
            let mut rejected = record.clone();
            rejected.stale_reason = Some(STALE_REVISION_REJECTED.to_owned());
            rejected.stale_at = Some(ts.to_owned());
            rejected.stale_detail.clone_from(&outcome.reason);
            self.store.update_distilled_unit_if_unchanged(
                &record,
                &rejected,
                ctx.actor.as_str(),
            )?;
            return Ok(outcome);
        }
        self.audit_reconcile(bank, &record, candidate, true, &claim_counts, ctx)?;
        self.cross_reference(bank, &candidate.id, &record.hub_id, claim_sources, ctx)?;
        Ok(DistillOutcome {
            hub_id: record.hub_id,
            item_id: candidate.id.clone(),
            status: "revised".into(),
            admission_score: Some(decision.admission_score),
            reason: None,
        })
    }

    /// The claim-set source items, in claim order: a unit whose claim set is
    /// settled rebuilds from the record's own ids only; any other unit's claim set
    /// grows with the recomputed selection, never shrinks (only the discard
    /// cascade removes ids, and it settles the claim set when it does). The unit's
    /// own lineage ids are excluded from the growth: its cross-reference and
    /// co-activation edges name itself, and a unit must never claim itself. At
    /// first distillation the lineage has no edges, so the selection is unchanged
    /// there. Unreadable sources are skipped and drop out of the rebuilt claim set.
    fn claim_source_items(
        &self,
        bank: &str,
        record: &DistilledUnitRecord,
        policy: &AssocPolicy,
    ) -> Result<Vec<MemoryItem>, AmError> {
        let lineage: std::collections::BTreeSet<String> = (1..=record.revision)
            .map(|n| clio_store::revision_item_id(&record.item_id, n))
            .collect();
        let mut claim: Vec<String> = record.source_ids.clone();
        // Growth is gated on the claim set being unsettled, not on the unit being
        // unmarked: a unit whose source was merely edited keeps growing with the
        // recomputed selection, because an edit removed no id.
        if !record.claim_set_settled() {
            for neighbor in self.top_neighbors(bank, &record.hub_id, policy)? {
                if !claim.contains(&neighbor.id) && !lineage.contains(&neighbor.id) {
                    claim.push(neighbor.id);
                }
            }
        }
        let mut out = Vec::new();
        for id in claim {
            match self.store.get_memory_item(bank, &id) {
                Ok(Some(item)) => out.push(item),
                Ok(None) => {}
                Err(err) if erasure_dead(&err) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(out)
    }

    /// The reconciliation audit row: the per-item anonymized-derivative
    /// determination for this distillation event, with the before/after claim
    /// counts. Ids and counts only, never text.
    fn audit_reconcile(
        &self,
        bank: &str,
        record: &DistilledUnitRecord,
        candidate: &MemoryItem,
        admitted: bool,
        claim_counts: &(usize, usize),
        ctx: &OpContext,
    ) -> Result<(), AmError> {
        let (claim_before, claim_after) = *claim_counts;
        let detail = serde_json::json!({
            "hub_id": record.hub_id,
            "item_id": candidate.id,
            "revision": record.revision + 1,
            "admitted": admitted,
            "claim_before": claim_before,
            "claim_after": claim_after,
            "derived": true,
        });
        self.store.record_assoc_audit(
            bank,
            "hub_distill_reconcile",
            Some(record.item_id.as_str()),
            ctx.actor.as_str(),
            &detail,
        )
    }
}

/// A row whose content erasure made unreadable (purged or shredded subject):
/// the unit is dead, and reconciliation must never resurrect it.
pub(super) fn erasure_dead(err: &AmError) -> bool {
    matches!(
        err.code,
        ErrorCode::UnreadableContent | ErrorCode::ErasedSubject
    )
}

/// Whether the unit changed: its claim set grew, or the candidate text differs
/// from the current text (the hub's gist moved, or a source's gist moved).
fn reconcile_changed(
    record: &DistilledUnitRecord,
    updated: &DistilledUnitRecord,
    current: &MemoryItem,
    candidate: &MemoryItem,
) -> bool {
    record.claim_ids() != updated.claim_ids() || candidate.gist != current.gist
}

#[path = "hub_distill_reconcile_outcomes.rs"]
mod hub_distill_reconcile_outcomes;

#[cfg(test)]
#[path = "hub_distill_reconcile_stamp_tests.rs"]
mod hub_distill_reconcile_stamp_tests;
