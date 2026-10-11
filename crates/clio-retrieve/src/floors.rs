// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the optional per-stage score floors a caller
//! can request on a retrieval: the request parameter type, its validation, and
//! the four floor predicates (one per named score), plus the provenance floor's
//! rung predicate.
//!
//! ## Owns
//! - `MinScores`, the optional `semantic` / `keyword` / `reranker` / `final`
//!   floors, plus `min_scores_from_value` and `MinScores::validate`
//! - `MAX_FLOOR` and the per-field ceiling rule
//! - `apply_semantic_floor`, `apply_keyword_floor`,
//!   `apply_reranker_floor`, and `final_floor_keeps`
//! - `binding_floor_warnings`, the wire-name warning for a floor that emptied
//!   the page
//! - `provenance_rung` and `provenance_floor_keeps`, the rung a provenance
//!   class sits at and whether an item clears the named floor
//!
//! ## Does not own
//! - Which stage each floor runs at (the orchestrator `hybrid` and the page
//!   mapper `finalize` place them)
//! - Score computation and the dense transform (`stage_score`), fusion math
//!   (`fusion`), and the rerank call (`rerank`)
//! - The provenance ladder's names, wire values, and strength order, which the
//!   retention level enum owns, and where a caller's floor is resolved from
//! - Parsing the surrounding request arguments (the bindings own that)
//!
//! ## Boundary
//!
//! Changes to a floor's meaning, validation, or the arm-local predicates should
//! normally require changes here. Changes to where a floor is applied, or to the
//! scores themselves, should not require changes here.
//!
//! # Arm-local floors, not a global relevance threshold
//!
//! `semantic` and `keyword` are **arm-local**: each one prunes only the arm it
//! names, before fusion, so the other arm's candidates are untouched. A
//! candidate the dense arm returned below the `semantic` floor leaves the
//! result set entirely unless the lexical arm also returned it. A hit that only
//! the lexical arm returned reports `semantic: null` and is unaffected by
//! `min_scores.semantic`; a graph-expanded candidate carries no arm score at
//! all and is likewise unaffected by both. Setting both floors therefore does
//! not restrict the response to hits that clear both — that is deliberate, and
//! an intersection would discard exactly the multi-arm hits fusion exists to
//! find.
//!
//! `reranker` and `final` are **not** arm-local: they are post-ranking
//! predicates, so every returned hit clears them. A candidate the reranker
//! never scored (no reranker configured, a fail-open rerank error, or a
//! document the provider omitted) is **kept** under a `reranker` floor, because
//! there is no value to compare; the floor can only remove a candidate the
//! reranker actually scored below it.
//!
//! # Only removes, and promises no page length
//!
//! Every floor only removes candidates, so no floor widens the caller's scope
//! or exceeds the token budget. None of them promises a shorter page: pruning
//! renumbers the surviving candidates, and the limit, the token budget and
//! duplicate suppression all run afterwards, so the room a removed hit leaves
//! can go to another hit. `final` is the one floor that cannot refill, because
//! it filters a page the limit and the budget already bounded.
//!
//! # Floors are relative, never calibrated
//!
//! `final` is a weighted-RRF sum and `keyword` is a raw backend score, so both
//! are relative values: they are meaningful only against the scores of the same
//! query and bank, and neither is a relevance probability. A floor is a caller
//! filter, not a quality promise; see the `scores` docs for each scale.
//!
//! Keep this module focused on the floors themselves.

use crate::fusion::StageHit;
use crate::stage_score::semantic_from_distance;
use clio_config::retention_levels::ProvenanceFloorDefault;
use clio_types::{AmError, EpistemicKind, ErrorCode, MemoryItem, SourceType};
use serde::Deserialize;
use serde_json::Value;

/// Largest accepted floor for the unbounded scales (`final`, `keyword`).
///
/// No score this system produces approaches it. The bound exists so garbage
/// input fails closed instead of silently meaning "return nothing"; it is not
/// a relevance threshold.
pub const MAX_FLOOR: f64 = 1_000_000.0;

/// Optional per-stage score floors. Every field is inclusive (`>=`) and an
/// absent field imposes no floor, so the default is a no-op.
///
/// `semantic` and `keyword` prune one retrieval arm each before fusion;
/// `reranker` and `final` filter the ranked result. See the module docs for
/// the exact semantics of each field.
///
/// # An unknown key is rejected, not ignored
///
/// `deny_unknown_fields` makes a mistyped key (`finall`, `semantic_typo`) fail
/// closed as `invalid_argument`, which is what the published `retrieve` schema
/// already promises with `additionalProperties: false`. Ignoring it would hand
/// the caller an unfiltered result while it believed the floor applied, which is
/// the one outcome a floor must never produce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinScores {
    /// Floor on the dense arm's documented cosine similarity, the same scale as
    /// a hit's `scores.semantic`. Prunes the dense arm only.
    pub semantic: Option<f64>,
    /// Floor on the lexical arm's raw backend score (SQLite `-bm25`, Postgres
    /// `ts_rank_cd`), the same scale as a hit's `scores.keyword`. Backend-
    /// dependent by construction: no cross-backend parity is claimed. Prunes
    /// the lexical arm only.
    pub keyword: Option<f64>,
    /// Floor on the reranker's normalized relevance, the same scale as a hit's
    /// `scores.reranker` (`0..=1`). Applied to the ranked result; a candidate
    /// the reranker did not score is kept.
    pub reranker: Option<f64>,
    /// Floor on the fused score of the returned page, the same scale as a hit's
    /// `scores.final`. Applied to the returned set after the limit and budget
    /// are applied, so it never backfills a freed slot.
    #[serde(rename = "final")]
    pub final_score: Option<f64>,
}

impl MinScores {
    /// True when no floor is set, so every filter is a no-op.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.semantic.is_none()
            && self.keyword.is_none()
            && self.reranker.is_none()
            && self.final_score.is_none()
    }

    /// Reject a floor that cannot be honored or that is nonsense input.
    ///
    /// A floor must be finite and non-negative. `semantic` and `reranker` live
    /// in `[0, 1]`, so a larger value could never be met and is rejected
    /// instead of silently emptying the result; the unbounded scales are
    /// rejected above [`MAX_FLOOR`].
    ///
    /// # Errors
    /// A non-finite, negative, or out-of-range floor value.
    pub fn validate(&self) -> Result<(), AmError> {
        for (name, floor) in self.fields() {
            let Some(floor) = floor else { continue };
            if !floor.is_finite() {
                return Err(invalid(format!(
                    "retrieve min_scores.{name} must be a finite number"
                )));
            }
            if floor < 0.0 {
                return Err(invalid(format!("retrieve min_scores.{name} must be >= 0")));
            }
            let max = field_max(name);
            if floor > max {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("retrieve min_scores.{name} must be <= {max}"),
                ));
            }
        }
        Ok(())
    }

    /// The four floors with their wire names, in schema order.
    fn fields(&self) -> [(&'static str, Option<f64>); 4] {
        [
            ("semantic", self.semantic),
            ("keyword", self.keyword),
            ("reranker", self.reranker),
            ("final", self.final_score),
        ]
    }
}

/// Parse and validate the optional `min_scores` argument of one request.
///
/// An absent or `null` value yields the empty (no-op) set. The same rules apply
/// whichever binding called this, so one request means one thing everywhere.
///
/// # Errors
/// A malformed shape, a non-numeric field, or a floor that fails
/// [`MinScores::validate`].
pub fn min_scores_from_value(value: Option<&Value>) -> Result<MinScores, AmError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(MinScores::default());
    };
    let parsed: MinScores = serde_json::from_value(value.clone())
        .map_err(|e| invalid(format!("retrieve min_scores: {e}")))?;
    parsed.validate()?;
    Ok(parsed)
}

/// Keep only the dense-arm candidates whose documented similarity clears the
/// floor.
///
/// The comparison uses the same transform as the `scores.semantic` field
/// (`clamp(1 - distance, 0, 1)`), so a caller's floor and the value it reads
/// back are on one scale. An absent floor returns the list untouched.
pub fn apply_semantic_floor(hits: Vec<StageHit>, floor: Option<f64>) -> Vec<StageHit> {
    let Some(floor) = floor else { return hits };
    hits.into_iter()
        .filter(|hit| semantic_from_distance(hit.score) >= floor)
        .collect()
}

/// Keep only the lexical-arm candidates whose raw backend score clears the
/// floor. The comparison uses the same value as the `scores.keyword` field. An
/// absent floor returns the list untouched.
pub fn apply_keyword_floor(hits: Vec<StageHit>, floor: Option<f64>) -> Vec<StageHit> {
    let Some(floor) = floor else { return hits };
    hits.into_iter().filter(|hit| hit.score >= floor).collect()
}

/// Keep only the ranked candidates whose rerank score clears the floor.
///
/// A candidate with no rerank score is kept: the floor can only drop a
/// candidate the reranker scored, and absence is not a score. An absent floor
/// returns the list untouched.
pub fn apply_reranker_floor<T>(
    hits: Vec<T>,
    floor: Option<f64>,
    rerank_score: impl Fn(&T) -> Option<f64>,
) -> Vec<T> {
    let Some(floor) = floor else { return hits };
    hits.into_iter()
        .filter(|hit| rerank_score(hit).is_none_or(|score| score >= floor))
        .collect()
}

/// Whether one fused score clears the `final` floor. An absent floor keeps
/// every score, so the returned page is untouched by default.
#[must_use]
pub fn final_floor_keeps(score: f64, floor: Option<f64>) -> bool {
    floor.is_none_or(|floor| score >= floor)
}

/// One warning per candidate floor named in `binding`, naming the floor by its
/// wire name.
///
/// Each entry is one of `semantic`, `keyword`, `reranker`, or `final`. The
/// subject is the wire name (`min_scores.semantic`) rather than a phrase, so
/// the CLI can rewrite it to the flag the caller typed with the same vocabulary
/// it uses for the tool's error messages. The caller emits these only when the
/// page came back empty, so a floor that merely shortened a non-empty page
/// reports nothing.
#[must_use]
pub fn binding_floor_warnings(binding: &[&str]) -> Vec<String> {
    binding
        .iter()
        .map(|name| format!("min_scores.{name} removed every retrieval candidate."))
        .collect()
}

/// The rung an item's own provenance sits at.
///
/// A fact is `verified`, so it is inside every rung. A belief sits at the rung
/// named by its source type (`user_stated`, `third_party`, `agent_inferred`).
/// `None` is the one state with no rung: a belief carrying no source type, which
/// item validation refuses to store. It is reported as no rung rather than an
/// invented one, so a reader excludes it from every rung instead of guessing a
/// strength.
#[must_use]
pub fn provenance_rung(
    kind: EpistemicKind,
    source: Option<SourceType>,
) -> Option<ProvenanceFloorDefault> {
    match (kind, source) {
        (EpistemicKind::Fact, _) => Some(ProvenanceFloorDefault::Verified),
        (EpistemicKind::Belief, Some(SourceType::UserStated)) => {
            Some(ProvenanceFloorDefault::Stated)
        }
        (EpistemicKind::Belief, Some(SourceType::ThirdParty)) => {
            Some(ProvenanceFloorDefault::ThirdParty)
        }
        (EpistemicKind::Belief, Some(SourceType::AgentInferred)) => {
            Some(ProvenanceFloorDefault::Inferred)
        }
        (EpistemicKind::Belief, None) => None,
    }
}

/// Whether `item` clears the provenance floor.
///
/// An absent floor keeps every item, so a request that names no floor behaves
/// exactly as it did before the floor existed. A named floor admits an item
/// whose rung is at or below it: the ladder runs from the strictest rung
/// (`verified`) to the loosest (`inferred`), so a `verified` floor keeps facts
/// only and an `inferred` floor keeps everything. An item with no rung is
/// excluded from every rung, so a floor can never widen itself around it.
///
/// Membership is the whole decision: no score, rank, budget, or stored field
/// takes part in it. `hybrid_fetch` prunes with it at fetch time, before
/// fusion, so an excluded item is absent from the page and from what
/// co-activation reinforced, and `dense_candidates`, `lexical_candidates` and
/// `fused_candidates` count what survived the prune. A floored page is not
/// promised to be shorter: the limit, the token budget and duplicate
/// suppression all run after it.
///
/// Pure: it reads only the item's own epistemic fields, so no clock, no
/// storage, and no configuration take part in the decision.
#[must_use]
pub fn provenance_floor_keeps(item: &MemoryItem, floor: Option<ProvenanceFloorDefault>) -> bool {
    floor.is_none_or(|floor| {
        provenance_rung(item.epistemic_kind, item.source_type).is_some_and(|rung| rung <= floor)
    })
}

/// The largest value each named floor may hold.
fn field_max(name: &str) -> f64 {
    match name {
        "semantic" | "reranker" => 1.0,
        _ => MAX_FLOOR,
    }
}

fn invalid(message: String) -> AmError {
    AmError::new(ErrorCode::InvalidArgument, message)
}

#[cfg(test)]
#[path = "floors_tests.rs"]
mod floors_tests;

#[cfg(test)]
#[path = "floors_ranking_tests.rs"]
mod floors_ranking_tests;

#[cfg(test)]
#[path = "floors_reinforcement_tests.rs"]
mod floors_reinforcement_tests;

#[cfg(test)]
#[path = "floors_integration_tests.rs"]
pub(crate) mod floors_integration_tests;

#[cfg(test)]
#[path = "floors_provenance_tests.rs"]
pub(crate) mod floors_provenance_tests;

#[cfg(test)]
#[path = "floors_provenance_surface_tests.rs"]
mod floors_provenance_surface_tests;
