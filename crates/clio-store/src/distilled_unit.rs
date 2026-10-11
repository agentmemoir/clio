// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hub-distilled derivation record shape and
//! its id-only claim-set and evidence rules.
//!
//! ## Owns
//! - `DISTILLED_UNIT_RECORD_VERSION` and `DistilledUnitRecord`
//! - `HUB_DISTILL_EDGE_RELATIONSHIP`, the association-edge relationship the
//!   writer emits for a distilled unit and the erasure purge matches
//! - `STALE_SOURCE_REMOVED` / `STALE_REVISION_REJECTED` / `STALE_SOURCE_CHANGED`,
//!   the stale reason codes
//! - Claim-set dedup, evidence recording, the proof count, and record validation
//!
//! ## Does not own
//! - The stored-value codec, re-exported from `distilled_unit_codec`
//! - Backend SQL (`sqlite_distilled_unit` / `postgres_distilled_unit`)
//! - Building records from a hub cluster (`clio-write`)
//! - The evidence entry shapes and the freshness decision
//!   (`distilled_evidence` / `distilled_freshness`)
//! - Revision identity and the item snapshot projection (`distilled_revision`)
//!
//! ## Boundary
//!
//! Changes to the record shape or its versioning should normally require
//! changes here. Changes to revision identity should require
//! `distilled_revision`; changes to the stored encoding should require
//! `distilled_unit_codec`; changes to backend SQL should not require changes here.
//!
//! Keep this module focused on the derivation record.

use crate::distilled_evidence::RevisionEvidence;
pub use crate::distilled_revision::revision_item_id;
pub(crate) use crate::distilled_unit_codec::{
    decode_source_ids, encode_source_ids, record_version_from_i64, revision_from_i64,
};
use clio_types::{AmError, ErrorCode, MemoryItem};
use std::collections::BTreeSet;

/// Version of the derivation record shape (the extension point for later
/// fields: a writer may add columns and bump this without a second structure).
pub const DISTILLED_UNIT_RECORD_VERSION: u32 = 3;

/// Stale reason written by the operations-removal cascade: a claimed source
/// left active use via `discard` (direct or hygiene). A unit stale for this
/// reason is not current in recall until reconciliation rebuilds it.
pub const STALE_SOURCE_REMOVED: &str = "source_removed";

/// Stale reason written when admission refused a re-derived revision: the prior
/// revision stays current in the store and the unit is returned, not withheld.
pub const STALE_REVISION_REJECTED: &str = "revision_rejected";

/// Stale reason for a claimed source's last change being newer than the unit's
/// last reconciliation, so the summary predates what it summarizes.
///
/// Decided on read, and **recorded on the record** by the wave that found it, so
/// the retrieve path can see it from the snapshot it already reads. An edit is
/// not an instruction to hide anything, so a unit stale for this reason is still
/// returned by `consolidated_only`; what it changes is that it stops suppressing
/// the raw items it may no longer represent. `get`/`inspect` see the lapse
/// immediately from the read-time comparison, whether or not a wave has recorded
/// it yet.
pub const STALE_SOURCE_CHANGED: &str = "source_changed";

/// Association-edge relationship from a distilled unit to each quoted source.
///
/// The writer emits this relationship for its cross-reference edges and both
/// erasure purge closures match it, so the three sides must hold one literal.
/// The maintenance job kind in `clio-write` currently shares this value; the
/// cross-crate parity test pins them equal.
pub const HUB_DISTILL_EDGE_RELATIONSHIP: &str = "hub_distill";

/// Durable derivation record for one hub-distilled item.
///
/// Keyed by the distilled item's own id and never resolved through
/// `(bank, subject)`. It carries ids and timestamps only: no content, gist, or
/// key material. The record is the authority for the claim set (`hub_id` plus
/// `source_ids`); the item snapshot is a projection written from the same
/// cluster state in the same store operation, so the two cannot drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistilledUnitRecord {
    /// Bank isolation key.
    pub bank_id: String,
    /// Distilled item id this record belongs to.
    pub item_id: String,
    /// Hub item id the cluster was distilled from.
    pub hub_id: String,
    /// Source item ids of the current claim set (deduplicated).
    pub source_ids: Vec<String>,
    /// Provenance marker (e.g. `hub_distilled`).
    pub marker: String,
    /// Record shape version (see [`DISTILLED_UNIT_RECORD_VERSION`]).
    pub record_version: u32,
    /// Distillation timestamp, shared with the item's own validity stamp.
    pub distilled_at: String,
    /// Revision number of the current item revision; 1 for the first write,
    /// incremented by every reconciliation revision. Pre-v2 rows read as 1.
    pub revision: u32,
    /// Stale reason when the unit is not current: [`STALE_SOURCE_REMOVED`],
    /// [`STALE_REVISION_REJECTED`], or [`STALE_SOURCE_CHANGED`]. The first two
    /// are determinations the store or an admission refusal make; the third is
    /// what a wave records when a claimed source moved after the last
    /// reconciliation. Each constant's own doc says which. `None` = the unit is
    /// current.
    pub stale_reason: Option<String>,
    /// Rejection or removal detail code when stale (ids and codes only).
    pub stale_detail: Option<String>,
    /// When the unit became stale; `None` while current.
    pub stale_at: Option<String>,
    /// The per-revision evidence trail, in revision order (record shape v3).
    /// Pre-v3 rows carry an empty trail: the list starts with the next revision
    /// a wave writes, and a no-op wave deliberately appends nothing.
    pub evidence: Vec<RevisionEvidence>,
}

impl DistilledUnitRecord {
    /// Build a record; deduplicates `source_ids` preserving order and drops
    /// empty ids. `record_version` starts at [`DISTILLED_UNIT_RECORD_VERSION`].
    #[must_use]
    pub fn new(
        bank_id: impl Into<String>,
        item_id: impl Into<String>,
        hub_id: impl Into<String>,
        source_ids: impl IntoIterator<Item = String>,
        marker: impl Into<String>,
        distilled_at: impl Into<String>,
    ) -> Self {
        let mut seen = BTreeSet::new();
        let source_ids = source_ids
            .into_iter()
            .filter(|id| !id.is_empty() && seen.insert(id.clone()))
            .collect();
        Self {
            bank_id: bank_id.into(),
            item_id: item_id.into(),
            hub_id: hub_id.into(),
            source_ids,
            marker: marker.into(),
            record_version: DISTILLED_UNIT_RECORD_VERSION,
            distilled_at: distilled_at.into(),
            revision: 1,
            stale_reason: None,
            stale_detail: None,
            stale_at: None,
            evidence: Vec::new(),
        }
    }

    /// Record the evidence for the record's **current** revision, appending one
    /// entry per claimed source.
    ///
    /// Append-only per revision, and idempotent within a revision: re-recording
    /// the same revision replaces that revision's list in place instead of
    /// growing a second copy of it, so a retried write cannot inflate a proof
    /// count. Recording a later revision never touches an earlier one, so the
    /// history of what was claimed then stays readable.
    pub fn record_evidence(
        &mut self,
        mut entries: Vec<crate::distilled_evidence::EvidenceEntry>,
        reconciled_at: &str,
    ) {
        // Stable by source id, so a proof count and a comparison of two lists
        // cannot depend on the order a cluster scan happened to return.
        entries.sort_by(|a, b| a.source_id.cmp(&b.source_id));
        let list = RevisionEvidence {
            revision: self.revision,
            reconciled_at: reconciled_at.to_owned(),
            entries,
        };
        match self
            .evidence
            .iter_mut()
            .find(|e| e.revision == self.revision)
        {
            Some(existing) => existing.clone_from(&list),
            None => self.evidence.push(list),
        }
    }

    /// The proof count: the distinct sources in the current revision's evidence
    /// list. Zero for a unit whose current revision predates the trail.
    #[must_use]
    pub fn proof_count(&self) -> usize {
        self.evidence
            .iter()
            .find(|e| e.revision == self.revision)
            .map_or(0, |e| e.entries.len())
    }

    /// The current revision's evidence entries (empty when it has none).
    #[must_use]
    pub fn current_evidence(&self) -> &[crate::distilled_evidence::EvidenceEntry] {
        self.evidence
            .iter()
            .find(|e| e.revision == self.revision)
            .map_or(&[], |e| e.entries.as_slice())
    }

    /// The whole trail, oldest revision first.
    #[must_use]
    pub fn evidence_history(&self) -> &[RevisionEvidence] {
        &self.evidence
    }

    /// Drop `source_id` from the **current** revision's evidence list, as the
    /// operations-removal cascade does when a claimed source leaves active use.
    ///
    /// The cascade writes no revision, so the current list is the one that has
    /// to stop claiming a source that is gone; leaving it would keep the proof
    /// count counting a source the claim set has already dropped. Earlier
    /// revisions are history of what was claimed then, so they keep their entry:
    /// rewriting them would be the history rewrite the trail exists to prevent.
    pub fn drop_source_from_current_evidence(&mut self, source_id: &str) {
        if let Some(list) = self
            .evidence
            .iter_mut()
            .find(|list| list.revision == self.revision)
        {
            list.entries.retain(|entry| entry.source_id != source_id);
        }
    }

    /// Whether the record's stale reason means its claim set is **settled**: the
    /// unit summarizes exactly the ids the record names and no more.
    ///
    /// Two reasons settle it, and the third does not:
    ///
    /// - `source_removed`: the cascade dropped an id because the item left active
    ///   use, so the claim set is deliberately smaller than the recomputed
    ///   selection and a rebuild must not widen it back.
    /// - `revision_rejected`: the record holds the last admitted claim set, and
    ///   widening it would only produce another candidate for admission to refuse.
    /// - `source_changed`: an edit moved a source's stamp and removed nothing.
    ///   The claim set is still open, so the unit keeps growing with the
    ///   recomputed selection exactly as a current unit does.
    ///
    /// This is the question reconciliation actually asks. "Is any reason
    /// recorded?" is a different question, and answering it with this one is what
    /// would make an edit silently stop a unit from ever claiming a new neighbour.
    #[must_use]
    pub fn claim_set_settled(&self) -> bool {
        matches!(
            self.stale_reason.as_deref(),
            Some(STALE_SOURCE_REMOVED | STALE_REVISION_REJECTED)
        )
    }

    /// Record that a claimed source's last change outran this unit's last
    /// reconciliation, so recall and the read surfaces agree on the lapse.
    ///
    /// Written by the wave's freshness pass on the record it is already
    /// re-reading, through the same claim/stale update the discard cascade uses,
    /// which re-projects the current revision item's snapshot from this state in
    /// the same transaction. Without that projection a retrieve call, which reads
    /// only the snapshot, would not see the reason at all.
    ///
    /// Refuses to overwrite a reason already recorded: a removal and an
    /// admission refusal are determinations in their own right, and replacing
    /// one with a weaker "something moved" would be a downgrade. The next
    /// successful revision clears the state, because that write is a record the
    /// claim set was re-derived.
    pub fn mark_source_changed(&mut self, at: &str) -> bool {
        if self.stale_reason.is_some() {
            return false;
        }
        self.stale_reason = Some(STALE_SOURCE_CHANGED.to_owned());
        self.stale_at = Some(at.to_owned());
        self.stale_detail = None;
        true
    }

    /// Item id of the current revision: the unit id itself at revision 1, then
    /// `{unit_id}:v{revision}` for later revisions. The suffix is only ever
    /// formatted here, never parsed; the record is the identity authority.
    #[must_use]
    pub fn current_revision_item_id(&self) -> String {
        revision_item_id(&self.item_id, self.revision)
    }

    /// Item id of the prior revision; `None` at revision 1 (first write).
    #[must_use]
    pub fn prior_revision_item_id(&self) -> Option<String> {
        (self.revision > 1).then(|| revision_item_id(&self.item_id, self.revision - 1))
    }

    /// The current claim set: the hub id plus every source id, deduplicated.
    #[must_use]
    pub fn claim_ids(&self) -> BTreeSet<String> {
        let mut ids: BTreeSet<String> = self.source_ids.iter().cloned().collect();
        ids.insert(self.hub_id.clone());
        ids
    }

    /// # Errors
    /// Empty bank/item/hub/marker/distilled_at, a `record_version` that is
    /// zero or exceeds the Postgres integer range, a `revision` below one, a
    /// stale state that is incomplete or names an unknown reason, or an
    /// evidence trail that repeats a revision, names one past the record's own,
    /// repeats a source inside one revision, or carries an entry without an id.
    pub(crate) fn validate(&self) -> Result<(), AmError> {
        if self.bank_id.is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        if self.item_id.is_empty()
            || self.hub_id.is_empty()
            || self.marker.is_empty()
            || self.distilled_at.is_empty()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distilled unit record fields are required",
            ));
        }
        if self.record_version == 0 {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distilled unit record_version must be non-zero",
            ));
        }
        // Both backends must accept the same `record_version` ceiling: the
        // Postgres column is a signed 32-bit integer.
        if self.record_version > i32::MAX as u32 {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distilled unit record_version exceeds the Postgres integer range",
            ));
        }
        if self.revision == 0 || self.revision > i32::MAX as u32 {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distilled unit revision must be in 1..=i32::MAX",
            ));
        }
        for list in &self.evidence {
            list.validate()?;
            if list.revision > self.revision {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "distilled unit evidence names a revision beyond the record's own",
                ));
            }
        }
        // Append-only per revision: a trail that repeated a revision would let
        // two lists claim the same revision, and the proof count reads one.
        for pair in self.evidence.windows(2) {
            if pair[1].revision <= pair[0].revision {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "distilled unit evidence revisions must be strictly increasing",
                ));
            }
        }
        match self.stale_reason.as_deref() {
            None => {
                if self.stale_at.is_some() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "distilled unit stale_at requires a stale_reason",
                    ));
                }
            }
            // All three reasons are writable: the cascade and the refusal path
            // write the first two, and the wave's freshness pass writes this one
            // so recall can see a lapse it cannot compute for itself.
            Some(STALE_SOURCE_REMOVED | STALE_REVISION_REJECTED | STALE_SOURCE_CHANGED) => {
                if self.stale_at.is_none() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "distilled unit stale_reason requires stale_at",
                    ));
                }
            }
            Some(other) => {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("distilled unit stale_reason is unknown: {other}"),
                ));
            }
        }
        Ok(())
    }

    /// Validate the record and require it to describe `item`.
    ///
    /// # Errors
    /// [`Self::validate`] failures, a record whose bank/item is not the item's
    /// own, or a `marker` that contradicts the item's provenance: the item must
    /// carry `source_ref = marker`, so a missing, empty, or different marker is
    /// a mismatch.
    pub(crate) fn validate_for_item(&self, item: &MemoryItem) -> Result<(), AmError> {
        self.validate()?;
        if self.bank_id != item.bank_id || self.item_id != item.id {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distilled unit record does not match the item",
            ));
        }
        if item.source_ref.as_deref() != Some(self.marker.as_str()) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "distilled unit record marker does not match the item source_ref",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "distilled_unit_cas_tests.rs"]
mod distilled_unit_cas_tests;

#[cfg(test)]
#[path = "distilled_unit_tests.rs"]
pub(crate) mod distilled_unit_tests;

#[cfg(test)]
#[path = "distilled_unit_reconcile_tests.rs"]
pub(crate) mod distilled_unit_reconcile_tests;

#[cfg(test)]
#[path = "distilled_unit_discard_cascade_tests.rs"]
mod distilled_unit_discard_cascade_tests;

#[cfg(test)]
#[path = "distilled_unit_discard_hub_tests.rs"]
mod distilled_unit_discard_hub_tests;
