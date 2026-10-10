// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hub-distillation data shapes and the pure
//! helpers that build a distilled semantic item from a hub cluster.
//!
//! ## Owns
//! - `HUB_DISTILL_JOB`, `HUB_DISTILL_MARKER`, `DistillOutcome`, `HubDistillReport`
//! - `StaleTally`, the by-reason count of units that are not current
//! - `HubSignalSource`, `modal_category`, `build_distill_item`,
//!   `build_distill_record`, `claimed_evidence`, `neighbor_text`
//!
//! ## Does not own
//! - Hub detection / the distill pipeline (`hub_distill`) or storage (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to the distill report/item shape should normally require changes here.
//! Changes to detection or admission wiring should not require changes here.

use clio_admission::gated::SignalSource;
use clio_store::{
    DistilledUnitRecord, EvidenceEntry, STALE_REVISION_REJECTED, STALE_SOURCE_CHANGED,
    STALE_SOURCE_REMOVED, UnitFreshness, evidence_from_items, snapshot_projection,
};
use clio_types::{AmError, EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde::Serialize;
use std::collections::BTreeMap;

/// Maintenance job kind for hub distillation (distinct from `memtree_refresh`).
pub const HUB_DISTILL_JOB: &str = "hub_distill";

/// Provenance marker written into a distilled item's `source_ref`.
///
/// This is the recall contract — "this item is a consolidated unit" — and not
/// the maintenance job kind: [`HUB_DISTILL_JOB`] names the per-item audit row
/// and the explicit cross-reference edges. The read side holds its own copy of
/// this literal (`HUB_DISTILL_SOURCE_REF` in `clio-retrieve`), and one
/// cross-crate test pins the two equal, so a rename of either side is a visible
/// change rather than a silent recall regression.
pub const HUB_DISTILL_MARKER: &str = "hub_distilled";

/// Max neighbors folded into one distilled item.
pub const DEFAULT_HUB_NEIGHBORS: usize = 8;

/// One hub distillation attempt result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DistillOutcome {
    /// Hub item id.
    pub hub_id: String,
    /// Distilled item id: the first distillation's unit id, or the id of the
    /// revision a reconciliation wrote.
    pub item_id: String,
    /// `distilled` | `rejected` | `already_distilled` | `no_neighbors` for a
    /// first distillation; `revised` | `revision_rejected` |
    /// `reconciled_current` | `stale_hub_gone` | `unit_superseded` |
    /// `reconcile_no_neighbors` | `revision_failed` | `purged` for a
    /// reconciliation of an existing unit.
    pub status: String,
    /// Admission score when scoring completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admission_score: Option<f64>,
    /// Rejection reason when admission failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Aggregate report for one consolidate hub wave.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct HubDistillReport {
    /// Hub ids above the degree limit.
    pub eligible: Vec<String>,
    /// Distilled item ids that were admitted.
    pub distilled: Vec<String>,
    /// Hub ids whose distillation was rejected by admission.
    pub rejected: Vec<String>,
    /// Hub ids already distilled in a prior wave that carry no derivation
    /// record (pre-record rows), so reconciliation cannot visit them.
    pub skipped_existing: Vec<String>,
    /// Hub ids a reconciliation re-derived this wave.
    pub revised: Vec<String>,
    /// Hub ids whose reconciliation revision admission refused.
    pub reconcile_rejected: Vec<String>,
    /// Hub ids a reconciliation found current (no writes).
    pub reconcile_unchanged: Vec<String>,
    /// Hub ids reconciliation could not make current (stale hub, purged
    /// content, no remaining sources, or a revision write that could not
    /// land); they stay stale and readable.
    pub reconcile_stale_kept: Vec<String>,
    /// How many units in the bank are not current once this wave's writes have
    /// landed, by reason.
    pub stale_units: StaleTally,
}

/// Units that are not current after a wave, counted by reason.
///
/// This describes how current the bank is now, not what the wave did: a unit
/// the wave revised successfully is current and is not counted, and a unit that
/// is also named in [`HubDistillReport::reconcile_stale_kept`] may be counted
/// here as well, because the two answer different questions. Every count comes
/// from the store's one freshness rule, so a unit whose sources changed in a way
/// the wave's own text comparison did not notice is still reported.
///
/// `unknown` is not a stale reason. It is the honest answer when the store could
/// not compare a stamp; such a unit is counted as not current and never
/// reported as current.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StaleTally {
    /// A claimed source left active use (`source_removed`).
    pub source_removed: usize,
    /// A claimed source changed after the unit's last reconciliation
    /// (`source_changed`).
    pub source_changed: usize,
    /// Admission refused the re-derived revision (`revision_rejected`).
    pub revision_rejected: usize,
    /// The store could not tell whether the unit is current.
    pub unknown: usize,
}

impl StaleTally {
    /// Count one unit's read-time freshness under its own reason. A unit the
    /// store reports as current counts nowhere.
    pub fn add(&mut self, freshness: &UnitFreshness) {
        match (freshness.stale, freshness.stale_reason.as_deref()) {
            (Some(false), _) => {}
            (Some(true), Some(STALE_SOURCE_REMOVED)) => self.source_removed += 1,
            (Some(true), Some(STALE_SOURCE_CHANGED)) => self.source_changed += 1,
            (Some(true), Some(STALE_REVISION_REJECTED)) => self.revision_rejected += 1,
            // A reason this build does not know, or a stamp it could not place,
            // still counts as not current rather than silently reading as
            // current.
            _ => self.unknown += 1,
        }
    }

    /// How many units are not current.
    #[must_use]
    pub fn total(&self) -> usize {
        self.source_removed + self.source_changed + self.revision_rejected + self.unknown
    }
}

impl HubDistillReport {
    /// File one outcome under its report list, by status.
    ///
    /// `distilled` | `already_distilled` keep their first-distillation lists;
    /// reconciliation statuses land in the reconcile lists; anything else is
    /// a first-distillation admission reject.
    pub fn push_outcome(&mut self, outcome: DistillOutcome) {
        match outcome.status.as_str() {
            "distilled" => self.distilled.push(outcome.item_id),
            "already_distilled" => self.skipped_existing.push(outcome.hub_id),
            "revised" => self.revised.push(outcome.hub_id),
            "revision_rejected" => self.reconcile_rejected.push(outcome.hub_id),
            "reconciled_current" => self.reconcile_unchanged.push(outcome.hub_id),
            "stale_hub_gone"
            | "unit_superseded"
            | "purged"
            | "reconcile_no_neighbors"
            | "revision_failed" => self.reconcile_stale_kept.push(outcome.hub_id),
            _ => self.rejected.push(outcome.hub_id),
        }
    }

    /// Compact status string for `consolidate` responses.
    ///
    /// The existing `key=N` pairs keep their order and their names, so a
    /// reader that splits the string and looks one up by prefix still finds it;
    /// the freshness counts are appended after them.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "hub_distill: eligible={},distilled={},rejected={},existing={},revised={},revised_rejected={},unchanged={},stale_kept={},stale={},stale_source_removed={},stale_source_changed={},stale_revision_rejected={},stale_unknown={}",
            self.eligible.len(),
            self.distilled.len(),
            self.rejected.len(),
            self.skipped_existing.len(),
            self.revised.len(),
            self.reconcile_rejected.len(),
            self.reconcile_unchanged.len(),
            self.reconcile_stale_kept.len(),
            self.stale_units.total(),
            self.stale_units.source_removed,
            self.stale_units.source_changed,
            self.stale_units.revision_rejected,
            self.stale_units.unknown,
        )
    }
}

/// Neighbor-text signal source for the novelty factor during distillation.
#[derive(Debug)]
pub(crate) struct HubSignalSource {
    pub(crate) texts: Vec<String>,
}

impl SignalSource for HubSignalSource {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(self.texts.clone())
    }

    fn prior_age_days(&self, _item: &MemoryItem) -> Result<Option<f64>, AmError> {
        Ok(None)
    }
}

/// Count endpoints that will receive a cross-reference (hub + distinct neighbors).
pub(crate) fn endpoints_count(item_id: &str, hub_id: &str, neighbors: &[MemoryItem]) -> usize {
    let mut n = 1;
    for neighbor in neighbors {
        if neighbor.id != item_id && neighbor.id != hub_id {
            n += 1;
        }
    }
    n
}

/// Short lexical text for a neighbor (gist, else id).
pub(crate) fn neighbor_text(item: &MemoryItem) -> String {
    item.gist
        .clone()
        .unwrap_or_else(|| item.id.clone())
        .chars()
        .take(200)
        .collect()
}

/// Modal `SemanticCategory` across the hub + neighbors, defaulting to `Schema`.
pub(crate) fn modal_category(hub: &MemoryItem, neighbors: &[MemoryItem]) -> SemanticCategory {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for item in std::iter::once(hub).chain(neighbors.iter()) {
        if let Some(category) = item.category {
            *counts.entry(category.as_str()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .and_then(|(key, _)| SemanticCategory::parse(key).ok())
        .unwrap_or(SemanticCategory::Schema)
}

/// Build the derivation record for the distilled item from the same cluster
/// state its snapshot is projected from.
///
/// The record is keyed by the distilled item's own id and is the authority for
/// the claim set; the item snapshot is written from this same record in the
/// same store operation, so the two cannot drift. Revision 1's evidence list is
/// built here too, from the very same hub and neighbours, so the claim set and
/// the evidence that backs it cannot name different sources.
pub(crate) fn build_distill_record(
    bank: &str,
    hub: &MemoryItem,
    item_id: &str,
    neighbors: &[MemoryItem],
    ts: &str,
) -> DistilledUnitRecord {
    let mut record = DistilledUnitRecord::new(
        bank,
        item_id,
        hub.id.clone(),
        neighbors.iter().map(|n| n.id.clone()),
        HUB_DISTILL_MARKER,
        ts,
    );
    record.record_evidence(claimed_evidence(hub, neighbors), ts);
    record
}

/// One evidence entry per claimed source: the hub plus the neighbours the claim
/// set was derived from.
///
/// The one write-side rule, shared by the first distillation and every
/// reconciliation, so both land the same list shape. Ids and the source's own
/// last-change time only — no gist, no text, no quotes.
pub(crate) fn claimed_evidence(hub: &MemoryItem, sources: &[MemoryItem]) -> Vec<EvidenceEntry> {
    evidence_from_items(std::iter::once(hub).chain(sources.iter()))
}

/// Build the deterministic template-derived distilled semantic item.
///
/// `record` is the unit's derivation record (the record [`build_distill_record`]
/// built over the same cluster state, or the updated record a reconciliation
/// writes); `source_items` must be the record's claim-set sources in claim
/// order. The snapshot is the shared record projection, so record and snapshot
/// agree by construction (claim-set ids, counts, and stale state).
pub(crate) fn build_distill_item(
    bank: &str,
    hub: &MemoryItem,
    category: SemanticCategory,
    record: &DistilledUnitRecord,
    source_items: &[MemoryItem],
    ts: &str,
) -> MemoryItem {
    let hub_gist = hub.gist.as_deref().unwrap_or(&hub.id);
    let parts: Vec<String> = source_items
        .iter()
        .take(DEFAULT_HUB_NEIGHBORS)
        .map(neighbor_text)
        .collect();
    let gist = format!("hub_distill: {} related to {}", hub_gist, parts.join(" | "));
    MemoryItem {
        id: record.current_revision_item_id(),
        bank_id: bank.to_owned(),
        // A distilled item inherits the hub's data subject, so the hub's own
        // erasure closes the distilled item too. There is deliberately no
        // cross-subject refusal: every stored item's subject defaults to its own
        // id, so a cluster of plain writes always spans subjects, and a refusal
        // would turn distillation off on the default path. The protection that
        // exists is the erasure derivation closure: erasing any source subject
        // purges every derived item whose explicit cross-reference names one of
        // its items (`distilled_referrers` + `purge_derived_item` in the store's
        // erase SQL), pinned by a store regression test.
        subject_id: hub.subject_id.clone(),
        kind: ItemKind::Semantic,
        category: Some(category),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: Some(HUB_DISTILL_MARKER.to_owned()),
        snapshot: Some(snapshot_projection(record)),
        gist: Some(gist),
        valid_from: ts.to_owned(),
        valid_until: None,
        tx_from: ts.to_owned(),
        tx_until: None,
        created_at: ts.to_owned(),
        updated_at: ts.to_owned(),
        context: None,
    }
}
