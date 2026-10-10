// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for read-time recall narrowing: the consolidated
//! eligibility predicate, the consolidated-only scope filter, the
//! prefer-consolidated supersede rule, and the page-local near-duplicate cap.
//!
//! ## Owns
//! - `DedupConfig`, `DedupedCandidates`, `DedupReport`
//! - `is_consolidated_item` / `consolidated_source_ids` eligibility accessors
//! - `dedupe_candidates`, the scope + supersede + near-duplicate pass
//!
//! ## Does not own
//! - Ranking, fusion, or rerank (`hybrid` / `fusion` / `rerank`)
//! - Budget truncation and the returned page shape (`finalize`)
//! - Profile storage or the bank default resolution (callers pass `DedupConfig`)
//!
//! ## Boundary
//!
//! Changes to eligibility, the supersede rule, or the similarity cap should
//! normally require changes here. Changes to ranking or packing should not.
//!
//! Keep this module focused on read-time recall narrowing.
//!
//! # Consolidated eligibility and links
//!
//! The eligible units are hub-distilled semantic items admitted through the
//! category gate: the distillation job writes the provenance marker
//! [`HUB_DISTILL_SOURCE_REF`] into `source_ref` and records the hub id plus
//! neighbour item ids in its snapshot. The marker is a recall contract of its
//! own — the maintenance job's name is a separate constant on the write side —
//! and it is the only provenance literal the predicate accepts: the historical
//! `hub_distill` literal was retired once no bank still held an item row that
//! carries it. Canonical consolidation outputs are not eligible: that path
//! stores a comma-joined provenance string in `source_ref` and keeps chunk ids on
//! the ingest leaf, so a canonical item carries no durable consolidated marker
//! and no item-id source links. A bank whose consolidation is canonical only
//! therefore returns an empty `consolidated_only` page with `scope_empty = true`;
//! the near-duplicate cap is the coverage path there. The predicate and the link
//! accessor are ready for a writer-side provenance addition.
//!
//! # A unit whose source was removed
//!
//! Discarding an item a distilled unit quotes removes that item from the unit's
//! claim set and marks the unit stale with reason `source_removed`. A
//! `source_removed` unit is **not** eligible for `consolidated_only` and does
//! **not** suppress raw items under `prefer_consolidated`, so the next recall
//! backfills the freed slots with the raw sources the operator never withdrew.
//! Under `full` scope the unit is still returned, still carries
//! `consolidated: true`, and its snapshot names the reason, so an operator can
//! see why a summary is being withheld. The unit returns to eligibility when
//! the next consolidation wave rebuilds its text from the remaining claim set
//! and clears the stale state; the withdrawn item's text does not come back with
//! it. A unit whose hub, rather than a source, was discarded stays stale until
//! the hub comes back into active use.
//!
//! # A unit that is stale for any other reason
//!
//! Only a **removal** withholds a unit. A unit stale because a claimed source
//! changed (`source_changed`), or because admission refused a re-derived
//! revision (`revision_rejected`), is still returned by `consolidated_only`, and
//! **no stale unit suppresses a raw item**: a summary that predates what it
//! summarizes may no longer represent the item it claims, so suppressing that
//! item would hide the operator's fresh text behind a parent that does not
//! reflect it. A current unit suppresses its claimed raw items exactly as
//! before.
//!
//! The split follows operator intent. A removal is an instruction that the
//! content leave active use, so a summary repeating it must stop being current.
//! An edit is not an instruction to hide anything: the summary simply predates
//! the change, so the honest treatment is to return it labelled while making
//! sure the fresh raw item is not hidden behind it.
//!
//! A `source_changed` lapse is decided on read, and the wave that finds it also
//! writes it onto the record, so this path sees it from the same snapshot it
//! reads every other reason from — no extra read, no new data source. Between a
//! source's edit and the next wave the unit still reads as a current parent,
//! because the reason has not been written yet; that window is the accepted cost
//! of keeping the retrieve path snapshot-only. `get` and `inspect` see the lapse
//! immediately either way, and a successful revision clears it.
//!
//! # Page-aware narrowing
//!
//! Both narrowings observe the page the caller actually receives: a raw item is
//! suppressed only when a consolidated parent that survives into that page claims
//! it, and the near-duplicate cap compares page items only. Dropping an item frees
//! a slot for the next-best candidate, so the pass repeats until it drops nothing.
//! The cap uses a token-set Jaccard similarity identical to the admission novelty
//! proxy, so no new embedding dependency is introduced. Each repetition costs
//! O(page^2) comparisons, bounded by the leg fetch and the requested limit, never
//! by the whole bank.

use crate::dedup_cap::{NarrowPass, effective_cap, narrow_page};
use crate::fusion::FusedHit;
use clio_config::retention::{DuplicateTolerance, RecallScope};
use clio_types::{ItemKind, MemoryItem};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// `source_ref` provenance marker the hub-distillation job writes today.
///
/// This is the recall contract ("this item is a consolidated unit"). It is
/// deliberately not the maintenance job's name — the writer's `HUB_DISTILL_JOB`
/// names the audit row and the explicit cross-reference edges — and the writer
/// holds its own copy of this literal (`HUB_DISTILL_MARKER` in `clio-write`).
/// One cross-crate test pins the two equal, so renaming either side is a visible
/// change rather than a silent recall regression.
pub const HUB_DISTILL_SOURCE_REF: &str = "hub_distilled";

/// `stale_reason` value written into a distilled unit's snapshot by the
/// operations-removal cascade: a claimed source left active use via `discard`.
///
/// A unit stale for this reason is **not current** in recall: it is withheld
/// from `consolidated_only` and does not suppress a raw item under
/// `prefer_consolidated`, until reconciliation rebuilds it. This is the read
/// side's own copy of the write-site literal (`STALE_SOURCE_REMOVED` in
/// `clio-store`); one cross-crate test pins the two equal, so a rename of
/// either side is a visible change rather than a silent recall regression.
pub const STALE_SOURCE_REMOVED: &str = "source_removed";

/// Read-time scope and dedup configuration for one retrieval call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DedupConfig {
    /// Requested scope preset (already resolved against the bank default).
    pub scope: RecallScope,
    /// Drop raw items whose consolidated parent is in the candidate set.
    pub prefer_consolidated: bool,
    /// Per-bank read-time tolerance level (its own mapping, not the write one).
    pub tolerance: DuplicateTolerance,
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self {
            scope: RecallScope::Full,
            prefer_consolidated: false,
            tolerance: DuplicateTolerance::Balanced,
        }
    }
}

impl DedupConfig {
    /// Whether the near-duplicate cap runs for this call.
    #[must_use]
    pub fn read_cap_active(&self) -> bool {
        self.prefer_consolidated
            || self.scope == RecallScope::ConsolidatedOnly
            || self.tolerance != DuplicateTolerance::Balanced
    }
}

/// True when `item` is a consolidated unit eligible for `consolidated_only`.
///
/// Only the current marker matches. The historical `hub_distill` literal named
/// the maintenance job, not item provenance, and no bank holds an item row that
/// carries it, so a row marked with the old literal is not a consolidated unit.
#[must_use]
pub fn is_consolidated_item(item: &MemoryItem) -> bool {
    item.kind == ItemKind::Semantic && item.source_ref.as_deref() == Some(HUB_DISTILL_SOURCE_REF)
}

/// Source item ids a consolidated parent claims (empty when it has no links).
#[must_use]
pub fn consolidated_source_ids(item: &MemoryItem) -> BTreeSet<String> {
    let Some(map) = item.snapshot.as_ref().and_then(Value::as_object) else {
        return BTreeSet::new();
    };
    let mut ids = BTreeSet::new();
    if let Some(hub) = map
        .get("hub_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        ids.insert(hub.to_owned());
    }
    if let Some(neighbors) = map.get("neighbor_ids").and_then(Value::as_array) {
        for id in neighbors
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            ids.insert(id.to_owned());
        }
    }
    ids
}

/// True when a consolidated unit is not current because an operations removal
/// (cascade) removed one of its claimed sources.
///
/// A removal is an instruction that the content leave active use, so such a unit
/// is **withheld** from `consolidated_only`: a summary repeating withdrawn
/// content must stop being current. It is still returned under the `full` scope,
/// where its snapshot names the reason.
///
/// This is the only reason that withholds. The other reasons leave the unit
/// returned — see [`is_stale`] for what they change instead.
#[must_use]
pub fn is_source_removed_stale(item: &MemoryItem) -> bool {
    stale_reason(item) == Some(STALE_SOURCE_REMOVED)
}

/// True when a consolidated unit carries any stale state, whatever the reason.
///
/// **No stale unit suppresses a raw item.** A summary that predates what it
/// summarizes, or whose re-derivation admission refused, may no longer represent
/// the raw item it claims — so suppressing that item would hide the operator's
/// fresh text behind a parent that does not reflect it. A stale unit is still
/// returned (only [`is_source_removed_stale`] withholds), and the raw items it
/// claims come back beside it.
///
/// The reason is read from the snapshot, which is where the write path projects
/// it. Every reason this path can see therefore hides a source over the lapse it
/// names, including `source_changed`, which the wave records onto the record and
/// this snapshot re-projection carries.
#[must_use]
pub fn is_stale(item: &MemoryItem) -> bool {
    stale_reason(item).is_some_and(|reason| !reason.is_empty())
}

/// The `stale_reason` a consolidated unit's snapshot carries, if any.
fn stale_reason(item: &MemoryItem) -> Option<&str> {
    item.snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|map| map.get("stale_reason"))
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
}

/// PII-safe per-request dedup counters and id lists (ids only, never text).
///
/// Every counter here describes the scope/dedup stage, which runs before the
/// `final` score floor, so a count or id list can name an item the floor then
/// removed from the returned page. They are not a description of the final
/// hits.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DedupReport {
    /// Scope applied to this call (`full` | `consolidated_only`).
    pub scope_applied: String,
    /// True when `consolidated_only` matched nothing in the bank.
    pub scope_empty: bool,
    /// Fused candidates considered before any suppression.
    pub candidates_considered: usize,
    /// Raw items dropped because a parent on the stage's page claims them.
    ///
    /// A later `final` score floor can remove that parent, so the count can
    /// exceed what the returned page shows.
    pub raw_suppressed: usize,
    /// Items promoted into the page this stage built, by freed slots.
    ///
    /// A later `final` score floor can remove one of them from the returned
    /// page, so this counts the scope/dedup stage, not the final hits.
    pub backfilled: usize,
    /// True when fewer candidates remained than the suppression freed slots.
    pub backfill_exhausted: bool,
    /// Page items dropped because their text is identical to a kept item's.
    ///
    /// Always counted, at every tolerance level: this is the unconditional part
    /// of the cap. A non-zero value here means the bank holds repeated text,
    /// which is a write-side problem, not normal retrieval behaviour.
    pub exact_dup_suppressed: usize,
    /// Page items dropped by the near-duplicate cap above exact equality.
    ///
    /// Zero unless the bank's `duplicate_tolerance_read` widens the cap past
    /// exact equality, so this stays a measure of retrieval tuning rather than
    /// of bank hygiene.
    pub near_dup_suppressed: usize,
    /// Ids of the raw items the prefer-consolidated rule dropped.
    ///
    /// A later `final` score floor can remove the claiming parent, so these are
    /// the stage's own ids, not the final hit list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suppressed_raw_ids: Vec<String>,
    /// Ids promoted into the page this stage built, by freed slots.
    ///
    /// A later `final` score floor can remove one of them from the returned
    /// page, so this is the stage's own id list, not the final hit list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub backfilled_ids: Vec<String>,
}

impl Default for DedupReport {
    fn default() -> Self {
        Self {
            scope_applied: RecallScope::Full.as_str().to_owned(),
            scope_empty: false,
            candidates_considered: 0,
            raw_suppressed: 0,
            backfilled: 0,
            exact_dup_suppressed: 0,
            backfill_exhausted: false,
            near_dup_suppressed: 0,
            suppressed_raw_ids: Vec::new(),
            backfilled_ids: Vec::new(),
        }
    }
}

impl DedupReport {
    /// True when nothing was scoped or suppressed (plain default retrieval).
    ///
    /// Serialization is skipped for this case so a call without the new
    /// parameters is byte-identical to the earlier output.
    #[must_use]
    pub fn is_inert(&self) -> bool {
        self.scope_applied == RecallScope::Full.as_str()
            && !self.scope_empty
            && self.raw_suppressed == 0
            && self.backfilled == 0
            && self.exact_dup_suppressed == 0
            && self.near_dup_suppressed == 0
    }
}

/// Scope-filtered candidates plus the suppression bookkeeping finalize needs.
pub(crate) struct DedupedCandidates {
    pub(crate) hits: Vec<FusedHit>,
    /// Scope-filtered order before any suppression (the backfill baseline).
    pub(crate) scope_page: Vec<FusedHit>,
    pub(crate) meta: DedupMeta,
}

/// Suppression counts and ids the outcome report is built from.
pub(crate) struct DedupMeta {
    pub(crate) scope: RecallScope,
    pub(crate) scope_empty: bool,
    pub(crate) candidates_considered: usize,
    pub(crate) suppressed_raw_ids: Vec<String>,
    pub(crate) near_dup_suppressed: usize,
    pub(crate) exact_dup_suppressed: usize,
}

/// Apply the scope filter and the page-aware narrowing passes, preserving rank
/// order throughout.
pub(crate) fn dedupe_candidates(
    fused: Vec<FusedHit>,
    items: &BTreeMap<String, MemoryItem>,
    cfg: DedupConfig,
    limit: usize,
    budget: Option<usize>,
) -> DedupedCandidates {
    let candidates_considered = fused.len();
    let scope_page: Vec<FusedHit> = match cfg.scope {
        RecallScope::Full => fused,
        RecallScope::ConsolidatedOnly => fused
            .into_iter()
            .filter(|h| {
                items
                    .get(&h.id)
                    // A unit stale for source removal is not current: withheld
                    // from the scope until reconciliation rebuilds it.
                    .is_some_and(|item| {
                        is_consolidated_item(item) && !is_source_removed_stale(item)
                    })
            })
            .collect(),
    };
    let scope_empty = cfg.scope == RecallScope::ConsolidatedOnly && scope_page.is_empty();
    let mut filtered = scope_page.clone();

    let mut suppressed_raw_ids = Vec::new();
    let mut near_dup_suppressed = 0_usize;
    let mut exact_dup_suppressed = 0_usize;
    {
        let pass = NarrowPass {
            prefer_consolidated: cfg.prefer_consolidated,
            cap: effective_cap(cfg),
            limit,
            budget,
        };
        while narrow_page(
            &mut filtered,
            items,
            &pass,
            &mut suppressed_raw_ids,
            &mut near_dup_suppressed,
            &mut exact_dup_suppressed,
        ) {}
    }

    DedupedCandidates {
        hits: filtered,
        scope_page,
        meta: DedupMeta {
            scope: cfg.scope,
            scope_empty,
            candidates_considered,
            suppressed_raw_ids,
            near_dup_suppressed,
            exact_dup_suppressed,
        },
    }
}

#[cfg(test)]
#[path = "dedup_tests.rs"]
mod dedup_tests;

#[cfg(test)]
#[path = "dedup_page_tests.rs"]
mod dedup_page_tests;

#[cfg(test)]
#[path = "dedup_marker_tests.rs"]
mod dedup_marker_tests;

#[cfg(test)]
#[path = "dedup_stale_tests.rs"]
mod dedup_stale_tests;

#[cfg(test)]
#[path = "dedup_exact_duplicate_tests.rs"]
mod dedup_exact_duplicate_tests;
