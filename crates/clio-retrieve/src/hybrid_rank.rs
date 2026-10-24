// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the rank shaping stages of hybrid retrieval:
//! weighted RRF fusion, one importance-only shaping step, the optional floored
//! age factor, the bounded trust multiplier, graph-decayed extra candidates,
//! and the optional rerank.
//!
//! ## Owns
//! - `fuse_candidates`, the fusion + shaping + age + trust + graph assembly
//! - `trust_factor_for`, the provenance-to-ratio multiplier mapping
//! - `apply_rerank`, the bounded shortlist rerank call and fallback
//!
//! ## Does not own
//! - Candidate fetch/filter flow or scope/dedup/finalize (`hybrid`, `dedup`,
//!   `finalize`), backend search SQL (`clio-store`), or rerank transport (`rerank`)
//!
//! ## Boundary
//!
//! Changes to the shaping step or the rerank fallback should normally require
//! changes here. Changes to retrieval control flow should not require changes
//! here.
//!
//! Keep this module focused on rank shaping.

use crate::filter::age_days;
use crate::floors::provenance_rung;
use crate::fusion::{
    ArmSignal, DEFAULT_TEMPORAL_ARM_WEIGHT, FUSION_ARMS, FusedHit, RankList, StageHit, rrf_fuse,
};
use crate::hybrid_util::{doc_text, normalize_leg_weights, sort_fused};
use crate::rerank::Reranker;
use crate::types::DEFAULT_GRAPH_DECAY;
use clio_config::ranking::{RetrievalWeights, TrustRatios};
use clio_config::retention_levels::ProvenanceFloorDefault;
use clio_types::decay::floored_half_life_factor;
use clio_types::{EpistemicKind, MemoryItem, SourceType};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Request-scoped retention knobs for the age factor applied to fused hits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetentionArgs<'a> {
    /// Whether the per-bank retention curve is active. False leaves every
    /// score exactly as the importance-only shaping left it.
    pub(crate) active: bool,
    /// Curve floor in `(0, 1]`; read only when `active` is true.
    pub(crate) floor: f64,
    /// Half-life in days for the decaying share of the age factor.
    pub(crate) half_life_days: f64,
    /// The clock every age is measured against; fixed for the request.
    pub(crate) now: &'a str,
}

/// Whether the retention knobs are usable. A bad floor or half-life leaves
/// every score unchanged rather than zeroing or flooring them; the profile and
/// ranking-env validators are the rejecting boundary. `floor == 1.0` is valid
/// and leaves scores unchanged by the arithmetic. A non-positive half-life is
/// deliberately treated like the curve being off, not like the floor, because
/// demoting a score to the floor on a bad constant would punish the data for a
/// configuration error.
fn retention_usable(retention: RetentionArgs<'_>) -> bool {
    retention.active
        && retention.floor.is_finite()
        && retention.floor > 0.0
        && retention.floor <= 1.0
        && retention.half_life_days.is_finite()
        && retention.half_life_days > 0.0
}

/// Weighted RRF over the dense and lexical legs, plus the rank-only temporal
/// window list when one resolved, then one shaping step that multiplies each
/// fused score by `1 + w_importance * admission_score`, then graph-decayed
/// extra candidates, then the optional retention age factor over every hit;
/// returns the deterministic order.
///
/// The legs' raw stage scores ride along as per-arm provenance; only ranks
/// enter the fusion sum. The temporal list carries no scores at all: it is a
/// pure ranking, so nothing from it can become a raw-score cross-arm
/// comparison. With `temporal` empty the fusion is exactly the two search legs.
///
/// The shaping multiplier is this project's own step on top of the fusion, not
/// part of the RRF formula: a fused score is a sum of reciprocal ranks, and how
/// far an item's admission score should move that sum is a separate decision
/// from how the arms were fused.
///
/// The retention curve is a second, independent multiplier applied exactly once
/// to every returned hit, graph-expanded ones included. It reads each item's
/// `created_at`: a consolidated unit's revision write re-stamps `created_at` at
/// every rebuild, so that field is its last-rebuild time and the unit ages from
/// there. A hit whose item was not fetched, whose `created_at` does not parse,
/// or whose knobs are invalid keeps its score unchanged, so a bad ranking field
/// can never zero scores. The factor each hit received is carried on the hit
/// itself (`FusedHit.age_factor`), so reporting never recomputes it.
///
/// The trust multiplier is a third, independent multiplier applied exactly once
/// to every returned hit, graph-expanded ones included, after the age factor and
/// before the sort. It reads each fetched item's epistemic class through the
/// same mapping the provenance floor uses (`trust_factor_for`). A fact and a
/// hit whose item was not fetched keep `1.0`; an unusable ratio is neutralized
/// rather than allowed to punish the data. The factor each hit received is
/// carried on the hit itself (`FusedHit.trust_factor`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fuse_candidates(
    weights: &RetrievalWeights,
    fusion_k: usize,
    dense: &[StageHit],
    lexical: &[StageHit],
    temporal: &[String],
    items: &BTreeMap<String, MemoryItem>,
    expanded: &BTreeSet<String>,
    retention: RetentionArgs<'_>,
    trust: TrustRatios,
) -> Vec<FusedHit> {
    let (wd, wl) = normalize_leg_weights(weights);
    let (dense_ids, dense_scores) = split_leg(dense);
    let (lexical_ids, lexical_scores) = split_leg(lexical);
    let mut lists = vec![
        RankList {
            weight: wd,
            ids: &dense_ids,
            scores: Some(&dense_scores),
        },
        RankList {
            weight: wl,
            ids: &lexical_ids,
            scores: Some(&lexical_scores),
        },
    ];
    if !temporal.is_empty() {
        lists.push(RankList {
            weight: DEFAULT_TEMPORAL_ARM_WEIGHT,
            ids: temporal,
            scores: None,
        });
    }
    let mut fused = rrf_fuse(&lists, fusion_k);
    for h in &mut fused {
        if let Some(item) = items.get(&h.id) {
            let importance = item.admission_score.unwrap_or(0.0).clamp(0.0, 1.0);
            h.score *= 1.0 + weights.importance * importance;
        }
    }
    if !expanded.is_empty() && !fused.is_empty() {
        let best = fused[0].score;
        // One provenance slot per list actually fused, so a graph-only hit is
        // shaped like the hits around it.
        let arm_slots = fused[0].arms.len().max(FUSION_ARMS);
        for id in expanded {
            if items.contains_key(id) && !fused.iter().any(|h| &h.id == id) {
                fused.push(FusedHit {
                    id: id.clone(),
                    score: best * DEFAULT_GRAPH_DECAY,
                    arms: vec![ArmSignal::default(); arm_slots],
                    rerank_score: None,
                    age_factor: 1.0,
                    trust_factor: 1.0,
                });
            }
        }
    }
    // The curve runs after graph expansion and before the sort, exactly once
    // per returned hit. `created_at`, never `updated_at`: a consolidated unit's
    // revision write re-stamps `created_at` at every rebuild, so it is that
    // unit's last-rebuild time. Age is computed here and never persisted.
    if retention_usable(retention) {
        for h in &mut fused {
            let Some(item) = items.get(&h.id) else {
                continue;
            };
            let Some(age) = age_days(&item.created_at, retention.now) else {
                continue;
            };
            // Bound once and write it once: the multiplication and the carried
            // report use the same value, so they can never disagree.
            let factor = floored_half_life_factor(age, retention.half_life_days, retention.floor);
            h.score *= factor;
            h.age_factor = factor;
        }
    }
    // The trust multiplier runs after the age curve and before the sort. Every
    // hit starts neutral and this loop writes the factor exactly once, so a hit
    // can never receive it twice; graph-expanded hits already joined above and
    // receive it like any other hit. A hit whose item was not fetched keeps the
    // neutral `1.0`.
    for h in &mut fused {
        let Some(item) = items.get(&h.id) else {
            continue;
        };
        // Bound once and write it once: the multiplication and the carried
        // report use the same value, so they can never disagree.
        let factor = trust_factor_for(item.epistemic_kind, item.source_type, trust);
        h.score *= factor;
        h.trust_factor = factor;
    }
    sort_fused(&mut fused, items);
    fused
}

/// The trust multiplier one item's provenance earns, read through the same
/// class mapping the provenance floor uses ([`provenance_rung`]).
///
/// A fact (`Verified`) and the no-rung state keep `1.0`: a fact is not a weaker
/// statement, and a belief with no source type has no ratio declared for it. A
/// ratio that is non-finite or outside `(0, 1]` is neutralized to `1.0` by
/// [`usable_ratio`], mirroring the retention knobs: a bad knob never punishes
/// data. The ranking-env validator stays the rejecting boundary for stored
/// values.
pub(crate) fn trust_factor_for(
    kind: EpistemicKind,
    source: Option<SourceType>,
    trust: TrustRatios,
) -> f64 {
    match provenance_rung(kind, source) {
        Some(ProvenanceFloorDefault::Stated) => usable_ratio(trust.user_stated),
        Some(ProvenanceFloorDefault::ThirdParty) => usable_ratio(trust.third_party),
        Some(ProvenanceFloorDefault::Inferred) => usable_ratio(trust.agent_inferred),
        Some(ProvenanceFloorDefault::Verified) | None => 1.0,
    }
}

/// The ratio when it is usable, else the neutral `1.0`. Mirrors
/// `retention_usable`'s "a bad knob never punishes data" rule; the env
/// validator is the boundary that rejects such a value before it is stored.
fn usable_ratio(ratio: f64) -> f64 {
    if ratio.is_finite() && ratio > 0.0 && ratio <= 1.0 {
        ratio
    } else {
        1.0
    }
}

/// Split one leg's stage hits into parallel id and raw-score vectors.
fn split_leg(hits: &[StageHit]) -> (Vec<String>, Vec<f64>) {
    (
        hits.iter().map(|h| h.id.clone()).collect(),
        hits.iter().map(|h| h.score).collect(),
    )
}

/// Rerank the bounded shortlist in deployment-sized batches; a rerank error
/// keeps the fused order with a structured warning. On success each reordered
/// shortlist hit carries the provider's normalized relevance in `rerank_score`
/// (`None` for a document the provider omitted), and scored batches are merged
/// by that relevance. Returns the ordered candidates and whether rerank ran.
pub(crate) fn apply_rerank(
    reranker: Option<&Arc<dyn Reranker>>,
    shortlist_m: usize,
    query: &str,
    items: &BTreeMap<String, MemoryItem>,
    fused: Vec<FusedHit>,
    warnings: &mut Vec<String>,
) -> (Vec<FusedHit>, bool) {
    let Some(reranker) = reranker else {
        return (fused, false);
    };
    let shortlist: Vec<FusedHit> = fused.iter().take(shortlist_m).cloned().collect();
    if shortlist.is_empty() {
        return (fused, false);
    }
    let batch_size = reranker.max_batch_size();
    if batch_size == 0 {
        warnings.push("rerank unavailable, using fused order: batch size is zero".into());
        return (fused, false);
    }
    let mut reordered: Vec<FusedHit> = Vec::with_capacity(shortlist.len());
    for batch in shortlist.chunks(batch_size) {
        let docs: Vec<String> = batch.iter().map(|h| doc_text(items.get(&h.id))).collect();
        let pairs = match reranker.rerank(query, &docs, batch.len()) {
            Ok(pairs) => pairs,
            Err(e) => {
                warnings.push(format!(
                    "rerank unavailable, using fused order: {}",
                    e.message
                ));
                return (fused, false);
            }
        };
        for pair in &pairs {
            if let Some(h) = batch.get(pair.index) {
                let mut hit = h.clone();
                hit.rerank_score = pair.score;
                reordered.push(hit);
            }
        }
        for (i, h) in batch.iter().enumerate() {
            if !pairs.iter().any(|p| p.index == i) {
                // Defensive append; `rerank_score` stays `None`.
                reordered.push(h.clone());
            }
        }
    }
    // Scores are comparable within one provider, so merge independently
    // reranked batches by relevance. A single request keeps the provider's
    // established order, including custom rerankers that omit scores.
    if shortlist.len() > batch_size {
        reordered.sort_by(
            |left, right| match (left.rerank_score, right.rerank_score) {
                (Some(left), Some(right)) => right.partial_cmp(&left).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        );
    }
    let rest = fused.iter().skip(shortlist.len()).cloned();
    (reordered.into_iter().chain(rest).collect(), true)
}

#[cfg(test)]
#[path = "hybrid_rank_test_support.rs"]
mod hybrid_rank_test_support;

#[cfg(test)]
#[path = "hybrid_rank_tests.rs"]
mod hybrid_rank_tests;

#[cfg(test)]
#[path = "hybrid_rank_curve_tests.rs"]
mod hybrid_rank_curve_tests;

#[cfg(test)]
#[path = "hybrid_rank_curve_age_tests.rs"]
mod hybrid_rank_curve_age_tests;

#[cfg(test)]
#[path = "hybrid_rank_curve_trust_tests.rs"]
mod hybrid_rank_curve_trust_tests;
