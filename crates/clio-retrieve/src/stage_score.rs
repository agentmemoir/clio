// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-hit `scores` object surfaced on
//! retrieval hits: the `final` / `reranker` / `semantic` / `keyword` /
//! `age_factor` / `trust_factor` field contract and the
//! dense-distance-to-similarity transform.
//!
//! ## Owns
//! - `HitScores`, the `scores` object type and its field contract
//! - `semantic_from_distance`, the documented dense transform
//! - The documented per-backend lexical scale decision for `keyword`
//!
//! ## Does not own
//! - Candidate generation, fusion, or rerank (`hybrid`, `fusion`, `rerank`)
//! - Populating the fields on a hit (`finalize`) or binding serialization
//!   (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to the score-object shape or the dense transform should normally
//! require changes here. Changes to ranking or binding serialization should not
//! require changes here.
//!
//! # Score semantics
//!
//! `final` is the fused weighted-RRF score: relative, never calibrated, and
//! equal to the top-level `score` for the same hit. `reranker` is the
//! provider's relevance normalized to `[0, 1]`
//! (`rerank_parse::normalize_rerank_score`);
//! it is `null` when no reranker is configured, when the rerank failed
//! (fail-open), or for a document the provider omitted, and it is display
//! metadata only — not cross-provider comparable, never a replacement for
//! `final`. `semantic` is the dense cosine
//! similarity `clamp(1 - distance, 0, 1)`: an exact match is `1.0`, an
//! orthogonal pair is `0.0`, and a distance above `1` is clamped to `0.0` so no
//! negative value is emitted. `keyword` is the lexical arm's raw score exactly
//! as the backend returned it. `age_factor` is the retention factor that was
//! multiplied into `final` at the shaping site: a finite number in
//! `[floor, 1.0]`, always present, exactly `1.0` when the curve is disabled or
//! did not apply to the hit. It is carried on the fused hit from that shaping
//! site, never recomputed from timestamps here, and carries no content.
//! `trust_factor` is the trust multiplier multiplied into `final` at the same
//! shaping site: a finite number in `[smallest configured ratio, 1.0]`, always
//! present, and exactly `1.0` for a fact or when no ratio applied. It is
//! carried on the fused hit from that one site, never recomputed here, and it
//! is an order signal, not confidence: it never changes `epistemic_kind`,
//! `category`, or `confidence`.
//!
//! # Lexical scale is backend-dependent
//!
//! The lexical arm surfaces `-bm25(items_fts)` on SQLite (FTS5 `bm25()` ranks
//! lower-is-better and is negated on read) and `ts_rank_cd` on Postgres (which
//! already ranks higher-is-better and is not classic BM25). Both values are
//! "higher is better" within their backend, but their sign and scale differ, so
//! `keyword` is NOT comparable across backends and no parity is claimed. An
//! absent arm value is `null`, never a fabricated `0`.
//!
//! Keep this module focused on the per-hit score contract.

use serde::Serialize;

#[cfg(test)]
#[path = "stage_score_age_tests.rs"]
mod stage_score_age_tests;

#[cfg(test)]
#[path = "stage_score_trust_tests.rs"]
mod stage_score_trust_tests;

/// Dense cosine distance converted to a similarity in `[0, 1]`.
///
/// `clamp(1 - distance, 0.0, 1.0)`. The documented dense distance range is
/// `[0, 2]`, so a distance above `1` would otherwise produce a negative value;
/// the clamp floors it at `0.0`.
#[must_use]
pub fn semantic_from_distance(distance: f64) -> f64 {
    (1.0 - distance).clamp(0.0, 1.0)
}

/// Nested per-hit stage scores (the wire `scores` object).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HitScores {
    /// Fused weighted-RRF score; relative, never calibrated. Equals the
    /// top-level `score` for the same hit.
    #[serde(rename = "final")]
    pub final_score: f64,
    /// Provider rerank relevance normalized to `[0, 1]`; `null` when rerank
    /// did not score the hit (no reranker, a rerank error, or a document the
    /// provider omitted). Display metadata only: not cross-provider
    /// comparable and never a replacement for `final`.
    pub reranker: Option<f64>,
    /// Dense cosine similarity in `[0, 1]`; `null` when the dense arm did not
    /// score this hit.
    pub semantic: Option<f64>,
    /// Raw backend lexical score (SQLite `-bm25`, Postgres `ts_rank_cd`);
    /// `null` when the lexical arm did not score this hit. Not comparable
    /// across backends.
    pub keyword: Option<f64>,
    /// The retention factor that was applied to `final` at the shaping site: a
    /// finite number in `[floor, 1.0]`, always present for a returned hit, and
    /// exactly `1.0` when the curve is disabled or did not apply. Carried on
    /// the fused hit from the one site that computed it; never recomputed from
    /// timestamps here.
    pub age_factor: f64,
    /// The trust multiplier applied to `final` at the shaping site: a finite
    /// number in `[smallest configured ratio, 1.0]`, always present for a
    /// returned hit, and exactly `1.0` for a fact or when no ratio applied.
    /// Carried on the fused hit from the one site that computed it; never
    /// recomputed here. An order signal, not confidence: it never changes
    /// `epistemic_kind`, `category`, or `confidence`.
    pub trust_factor: f64,
}

#[cfg(test)]
#[path = "stage_score_tests.rs"]
mod stage_score_tests;
