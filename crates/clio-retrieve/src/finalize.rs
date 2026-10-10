// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for turning the fused (and optionally reranked and
//! deduped) candidate list into the bounded, ordered retrieval result.
//!
//! ## Owns
//! - `FinalizeArgs` and the final limit/budget/token-accounting step
//! - `select_page`, the limit + budget page selection shared with dedup
//! - The dedup report assembled into the outcome (`DedupReport`)
//! - Populating each `ScoredHit`'s `scores` fields from the per-arm signals,
//!   the reranker's normalized relevance, and the age and trust factors
//!   carried on the fused hit
//! - Attaching each hit's `entities[]` names from the fetched item's snapshot
//!   and the derived `entity_match` flag computed from the query and those names
//! - Applying the `final` score floor to the returned page, the entity-inclusion
//!   toggle, and the requested top-level entity summary
//! - Attaching each hit's optional bounded `exact_render` rendering from the
//!   item's own snapshot, only when the request asked for it
//!
//! ## Does not own
//! - Candidate generation, fusion, or rerank (owned by `hybrid`)
//! - Scope/dedup decisions and suppression (owned by `dedup`)
//! - The `HitScores` type, its field contract, and the dense transform
//!   (owned by `stage_score`)
//! - The entity-name projection and its empty-coverage rules (owned by
//!   `entities`), and the summary roll-up (owned by `entity_summary`)
//! - The floor predicates themselves (owned by `floors`)
//! - The rendering text and its byte cap (owned by `render`)
//!
//! ## Boundary
//!
//! Changes to final hit mapping or the result budget should normally require
//! changes here. Changes to candidate ranking should not require changes here.
//!
//! # The `final` floor applies to the returned set, after limit and budget
//!
//! The floor is applied to the page `select_page` already bounded, so it can
//! only shorten the result: a floored query may return fewer hits than `limit`
//! and never backfills a freed slot, widens scope, or exceeds the token budget
//! to compensate. The `dedup` report is computed from the same page before the
//! floor runs, so its counters describe the scope/dedup stage itself; a
//! `final` floor may then return fewer hits than that report counted.
//!
//! Keep this module focused on the bounded returned page.

use crate::dedup::{DedupMeta, DedupReport, consolidated_source_ids, is_consolidated_item};
use crate::entities::snapshot_entities;
use crate::entity_summary::summarize_entities;
use crate::floors::final_floor_keeps;
use crate::fusion::{ARM_DENSE, ARM_LEXICAL, ARM_TEMPORAL, FusedHit};
use crate::hybrid_util::doc_text;
use crate::render::render_exact;
use crate::stage_score::{HitScores, semantic_from_distance};
use crate::token::estimate_tokens;
use crate::types::{RetrieveMetrics, RetrieveOutcome, RetrieveRequest, ScoredHit};
use clio_types::{MemoryItem, RetrieveHit};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct FinalizeArgs<'a> {
    pub(crate) req: &'a RetrieveRequest,
    pub(crate) items: &'a BTreeMap<String, MemoryItem>,
    pub(crate) dense_rank: &'a BTreeMap<String, usize>,
    pub(crate) lexical_rank: &'a BTreeMap<String, usize>,
    pub(crate) fused: Vec<FusedHit>,
    pub(crate) dense_candidates: usize,
    pub(crate) lexical_candidates: usize,
    pub(crate) temporal_candidates: Option<usize>,
    pub(crate) reranked: bool,
    pub(crate) graph_expanded: usize,
    pub(crate) metrics: RetrieveMetrics,
    pub(crate) warnings: Vec<String>,
    pub(crate) dedup: DedupMeta,
    /// Page the scope filter would have returned before suppression; ids absent
    /// from it entered the result only because suppression freed slots.
    pub(crate) backfill_baseline: Vec<String>,
}

/// Budget policy: `budget_tokens` trims trailing hits once the running total
/// would exceed it, but the first hit is always admitted even when it alone
/// exceeds the budget — an empty result is never preferred over a relevant one.
/// The injection bound is still safe because `compose` re-truncates each hit's
/// composed text in `pack_memory`.
///
/// Returns the selected ids in rank order; the same function produces the
/// pre-suppression baseline used for the backfill report and for
/// reinforcement, so both observe one definition of the returned page.
pub(crate) fn select_page(
    fused: &[FusedHit],
    items: &BTreeMap<String, MemoryItem>,
    limit: usize,
    budget: Option<usize>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut estimated_tokens = 0_usize;
    for hit in fused.iter().take(limit) {
        let Some(item) = items.get(&hit.id) else {
            continue;
        };
        let tokens = estimate_tokens(&doc_text(Some(item)));
        if let Some(budget) = budget
            && estimated_tokens + tokens > budget
            && !ids.is_empty()
        {
            break;
        }
        estimated_tokens += tokens;
        ids.push(hit.id.clone());
    }
    ids
}

pub(crate) fn finalize(args: FinalizeArgs<'_>) -> RetrieveOutcome {
    let fused_candidates = args.fused.len();
    let page_ids = select_page(
        &args.fused,
        args.items,
        args.req.limit,
        args.req.budget_tokens,
    );
    let selected: BTreeSet<&String> = page_ids.iter().collect();
    let mut hits = Vec::with_capacity(page_ids.len());
    let mut estimated_tokens = 0_usize;
    for hit in args.fused.iter().filter(|h| selected.contains(&h.id)) {
        let Some(item) = args.items.get(&hit.id) else {
            continue;
        };
        // The `final` floor is the last step on the selected page: it removes
        // the hit and its tokens, and nothing refills the slot.
        if !final_floor_keeps(hit.score, args.req.min_scores.final_score) {
            continue;
        }
        estimated_tokens += estimate_tokens(&doc_text(Some(item)));
        let consolidated = is_consolidated_item(item);
        let semantic = hit
            .arms
            .get(ARM_DENSE)
            .and_then(|arm| arm.score)
            .map(semantic_from_distance);
        let keyword = hit.arms.get(ARM_LEXICAL).and_then(|arm| arm.score);
        // The toggle suppresses the names themselves, so the derived
        // `entity_match` flag has no names to compare and stays absent too.
        let entities = if args.req.include_entities {
            snapshot_entities(item)
        } else {
            Vec::new()
        };
        let entity_match = crate::entity_match::query_matches_any(&args.req.query, &entities);
        // The one construction site for the hit, so the rendering is attached
        // here. It reads the snapshot the assembler already holds on the item,
        // so it costs no storage read, and it is gated on the request's own
        // control so a request that does not ask produces nothing.
        let mut hit_fields = RetrieveHit::from_item(item);
        if args.req.include_exact {
            hit_fields.exact_render = item.snapshot.as_ref().and_then(render_exact);
        }
        hits.push(ScoredHit {
            hit: hit_fields,
            score: hit.score,
            scores: HitScores {
                final_score: hit.score,
                reranker: hit.rerank_score,
                semantic,
                keyword,
                // Carried from the shaping site; never recomputed here.
                age_factor: hit.age_factor,
                trust_factor: hit.trust_factor,
            },
            dense_rank: args.dense_rank.get(&hit.id).copied(),
            lexical_rank: args.lexical_rank.get(&hit.id).copied(),
            // The temporal rank is the hit's own rank in the temporal arm's
            // fusion list, so it cannot drift from the list that produced it.
            temporal_rank: hit.arms.get(ARM_TEMPORAL).and_then(|arm| arm.rank),
            snapshot_ref: item.snapshot.as_ref().map(|_| item.id.clone()),
            entities,
            entity_match,
            consolidated,
            source_ids: if consolidated {
                consolidated_source_ids(item).into_iter().collect()
            } else {
                Vec::new()
            },
        });
    }
    let baseline: BTreeSet<&String> = args.backfill_baseline.iter().collect();
    let backfilled_ids: Vec<String> = page_ids
        .iter()
        .filter(|id| !baseline.contains(id))
        .cloned()
        .collect();
    let suppression = args.dedup.suppressed_raw_ids.len()
        + args.dedup.exact_dup_suppressed
        + args.dedup.near_dup_suppressed;
    let backfill_exhausted = suppression > 0 && page_ids.len() < args.backfill_baseline.len();
    let dedup = DedupReport {
        scope_applied: args.dedup.scope.as_str().to_owned(),
        scope_empty: args.dedup.scope_empty,
        candidates_considered: args.dedup.candidates_considered,
        raw_suppressed: args.dedup.suppressed_raw_ids.len(),
        backfilled: backfilled_ids.len(),
        backfill_exhausted,
        near_dup_suppressed: args.dedup.near_dup_suppressed,
        exact_dup_suppressed: args.dedup.exact_dup_suppressed,
        suppressed_raw_ids: args.dedup.suppressed_raw_ids,
        backfilled_ids,
    };
    // Assembled from the returned hits, so it is rolled up from the same page
    // the caller sees. `validate` already rejected asking for a summary while
    // entity names are suppressed; the `include_entities` term keeps that true
    // for a caller that bypassed the binding check.
    let entity_summary =
        (args.req.entity_summary && args.req.include_entities).then(|| summarize_entities(&hits));
    RetrieveOutcome {
        hits,
        dense_candidates: args.dense_candidates,
        lexical_candidates: args.lexical_candidates,
        temporal_candidates: args.temporal_candidates,
        fused_candidates,
        reranked: args.reranked,
        graph_expanded: args.graph_expanded,
        pairs_reinforced: 0,
        estimated_tokens,
        metrics: args.metrics,
        warnings: args.warnings,
        dedup,
        entity_summary,
    }
}

#[cfg(test)]
#[path = "finalize_tests.rs"]
mod finalize_tests;
