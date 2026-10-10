// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for fusing multiple ranked candidate lists into
//! one deterministic order using weighted Reciprocal Rank Fusion.
//!
//! ## Owns
//! - `StageHit` / `RankList` / `ArmSignal` / `FusedHit` and the `rrf_fuse`
//!   algorithm
//!
//! ## Does not own
//! - Candidate generation (dense/lexical/graph legs live in `hybrid`/`hybrid_legs`)
//! - Importance boost or rerank (applied by the orchestrator)
//!
//! ## Boundary
//!
//! Changes to the fusion formula or tie-break should normally require changes
//! here. Changes to leg weights should not require changes here.
//!
//! # Weighted RRF contract
//!
//! `RRF_w(d) = Σ_i w_i / (k + rank_i(d))` with 1-based ranks and Cormack's
//! default `k = 60` ([`DEFAULT_RRF_K`]). When every `w_i = 1` this reduces to
//! the classic unweighted RRF from Cormack et al. (SIGIR 2009). Raw scores from
//! different legs (cosine distance vs `bm25()` / `ts_rank_cd`) are never mixed;
//! only ranks are. Ties break by id ascending here; the orchestrator may apply
//! an importance tie-break afterwards.
//!
//! # Per-arm provenance
//!
//! Each fused hit carries one [`ArmSignal`] per input list: the arm's 1-based
//! rank plus the arm's raw stage score when the caller supplied one. The score
//! is provenance only; it never enters the fusion sum.

use std::cmp::Ordering;
use std::collections::BTreeMap;

/// Cormack RRF default; near-optimal and not sacred.
pub const DEFAULT_RRF_K: usize = 60;

/// Fusion weight of the rank-only temporal arm.
///
/// Fixed at the default equal per-arm share of the two search legs, which
/// normalize to sum 1: the arm joins with the influence of one default leg and
/// no configuration knob changes the existing legs. It is not a calibrated
/// relevance value and never mixes a raw score — the arm contributes ranks
/// only.
pub const DEFAULT_TEMPORAL_ARM_WEIGHT: f64 = 0.5;

/// Fusion list order: the dense leg is list 0.
pub const ARM_DENSE: usize = 0;
/// Fusion list order: the lexical leg is list 1.
pub const ARM_LEXICAL: usize = 1;
/// Fusion list order: the temporal window arm is list 2 when it runs.
pub const ARM_TEMPORAL: usize = 2;
/// Number of always-present fusion arms: dense + lexical.
///
/// The rank-only temporal arm joins as an extra list only when a supported
/// expression resolved against an explicit anchor; a hit fused without the arm
/// carries two signals, a hit fused with it carries three.
pub const FUSION_ARMS: usize = 2;

/// One candidate from a single retrieval arm: item id plus its raw stage score.
///
/// Dense legs carry the cosine distance; lexical legs carry the backend's
/// lexical score. Order within a leg is best-first.
#[derive(Debug, Clone, PartialEq)]
pub struct StageHit {
    /// Candidate id.
    pub id: String,
    /// Raw stage score (cosine distance for dense; backend lexical score for
    /// lexical).
    pub score: f64,
}

/// Per-arm provenance for one fused hit: rank plus the raw stage score.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ArmSignal {
    /// 1-based rank in this arm's list (`None` when the arm did not return it).
    pub rank: Option<usize>,
    /// Raw stage score from this arm (`None` when the arm did not score it).
    pub score: Option<f64>,
}

/// One ranked candidate list with its fusion weight and optional raw scores.
#[derive(Debug, Clone)]
pub struct RankList<'a> {
    /// Non-negative weight; non-finite values are treated as `0.0`.
    pub weight: f64,
    /// Candidate ids best-first; duplicate ids keep their first rank.
    pub ids: &'a [String],
    /// Raw stage scores parallel to `ids` (`None` = rank-only list).
    pub scores: Option<&'a [f64]>,
}

/// One fused candidate with its per-arm provenance (parallel to the input lists).
#[derive(Debug, Clone, PartialEq)]
pub struct FusedHit {
    /// Candidate id.
    pub id: String,
    /// Weighted RRF score (higher = better).
    pub score: f64,
    /// Per-arm rank and raw score, in fusion list order.
    pub arms: Vec<ArmSignal>,
    /// Normalized rerank relevance in `[0, 1]` when the reranker scored this
    /// candidate; `None` otherwise (no reranker configured, a rerank error,
    /// or a document the provider omitted). Written by `apply_rerank`.
    pub rerank_score: Option<f64>,
    /// The retention age factor applied to `score` at the shaping site in
    /// `fuse_candidates`. Initialized to the neutral `1.0` here because fusion
    /// runs before shaping, so `rrf_fuse` never computes it; the shaping site
    /// is the only place the factor is computed and written.
    pub age_factor: f64,
    /// The trust multiplier applied to `score` at the shaping site in
    /// `fuse_candidates`. Initialized to the neutral `1.0` here because fusion
    /// runs before shaping, so `rrf_fuse` never computes it; the shaping site
    /// is the only place the factor is computed and written.
    pub trust_factor: f64,
}

impl FusedHit {
    /// Per-arm ranks in fusion list order (`None` when absent from that arm).
    #[must_use]
    pub fn ranks(&self) -> Vec<Option<usize>> {
        self.arms.iter().map(|arm| arm.rank).collect()
    }
}

/// Weighted RRF over `lists` with smoothing constant `k` (clamped to `>= 1`).
#[must_use]
pub fn rrf_fuse(lists: &[RankList<'_>], k: usize) -> Vec<FusedHit> {
    let k = k.max(1);
    let mut acc: BTreeMap<String, FusedHit> = BTreeMap::new();
    for (i, list) in lists.iter().enumerate() {
        let weight = if list.weight.is_finite() {
            list.weight.max(0.0)
        } else {
            0.0
        };
        for (rank0, id) in list.ids.iter().enumerate() {
            let rank = rank0 + 1;
            let entry = acc.entry(id.clone()).or_insert_with(|| FusedHit {
                id: id.clone(),
                score: 0.0,
                arms: vec![ArmSignal::default(); lists.len()],
                rerank_score: None,
                age_factor: 1.0,
                trust_factor: 1.0,
            });
            if entry.arms[i].rank.is_none() {
                entry.arms[i].rank = Some(rank);
                entry.arms[i].score = list.scores.and_then(|scores| scores.get(rank0)).copied();
                entry.score += weight / (k + rank) as f64;
            }
        }
    }
    let mut out: Vec<FusedHit> = acc.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

#[cfg(test)]
#[path = "fusion_tests.rs"]
mod fusion_tests;
