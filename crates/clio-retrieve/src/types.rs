// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the retrieval request/result DTOs and the
//! bounded graph-expansion seam shared by the hybrid orchestrator and the
//! context composer.
//!
//! ## Owns
//! - `RetrieveRequest`, `ScoredHit`, `RetrieveOutcome`, and tuning constants
//! - `GraphExpander` trait and the default no-op expander
//! - The request fields that carry the optional score floors
//!   (`min_scores`), the optional provenance floor, the entity-inclusion
//!   controls (`include_entities`, `entity_summary`), and the explicit
//!   relative-window anchor (`query_anchor`), plus the optional top-level
//!   `entity_summary` result
//! - The per-bank read-time retention knobs (`retention_curve`,
//!   `retention_floor`) carried from the resolved profile to shaping
//! - The `include_exact` opt-in that asks for each hit's bounded snapshot
//!   rendering
//! - The per-hit `temporal_rank` provenance and the optional
//!   `temporal_candidates` outcome counter
//!
//! ## Does not own
//! - The retrieval algorithm (`hybrid`), fusion math (`fusion`), or compose
//! - Relative-expression parsing and window resolution (`temporal`)
//! - The entity-name projection (`entities`) and the overlap match rule
//!   (`entity_match`)
//! - The floor rules and filters themselves (`floors`), and the summary
//!   assembly (`entity_summary`)
//!
//! ## Boundary
//!
//! Changes to the retrieval request/result contract should normally require
//! changes here. Changes to orchestration internals should not require changes here.
//!
//! # Optional controls are off by default
//!
//! `min_scores` defaults to no floor, `provenance_floor` to no floor, and
//! `entity_summary` to not requested, so a request that sets none of them
//! behaves exactly as it did before these fields existed. `include_exact`
//! defaults to `false`, so the default path produces no rendering and the hit
//! field is absent rather than empty. `include_entities`
//! defaults to `true`, which keeps the per-hit `entities[]` array on every hit.
//! `retention_curve` defaults to false and `retention_floor` to `0.25`, so a
//! request that sets neither keeps the order it had before the curve existed.
//!
//! Keep this module focused on the retrieval request/result contract.

use crate::dedup::DedupReport;
use crate::entity_summary::EntitySummary;
use crate::floors::MinScores;
use crate::stage_score::HitScores;
use clio_config::retention::{DuplicateTolerance, RecallScope};
use clio_config::retention_levels::ProvenanceFloorDefault;
use clio_types::{AmError, RetrieveHit, TimeAxis};
use serde::Serialize;

/// Default final result count.
pub const DEFAULT_LIMIT: usize = 10;
/// Default candidates fetched per leg before fusion.
pub const DEFAULT_LEG_FETCH: usize = 60;
/// Default fused shortlist size handed to the optional reranker.
pub const DEFAULT_SHORTLIST_M: usize = 60;
/// Score decay applied to graph-expanded candidates relative to their best seed.
pub const DEFAULT_GRAPH_DECAY: f64 = 0.5;

/// Bounded graph expansion seam.
///
/// Association-edge listing and co-activation writes are owned by the
/// co-activation subsystem; the default [`NoopGraphExpander`] returns no extra
/// candidates, so `expand_graph=true` degrades gracefully today.
pub trait GraphExpander: Send + Sync {
    /// Return additional candidate item ids reachable from `seeds` within `max_hops`.
    ///
    /// # Errors
    /// Traversal/storage failures.
    fn expand(&self, bank: &str, seeds: &[String], max_hops: usize)
    -> Result<Vec<String>, AmError>;
}

/// Default no-op expander (no association graph reads yet).
#[derive(Debug, Default)]
pub struct NoopGraphExpander;

impl GraphExpander for NoopGraphExpander {
    fn expand(
        &self,
        _bank: &str,
        _seeds: &[String],
        _max_hops: usize,
    ) -> Result<Vec<String>, AmError> {
        Ok(Vec::new())
    }
}

/// Arguments for one hybrid retrieval.
///
/// The boolean fields are independent opt-in request toggles with their own
/// documented defaults, not variants of one state, so the excessive-bools
/// heuristic does not apply: `expand_graph` bounds traversal,
/// `prefer_consolidated` selects a dedup mode, `retention_curve` selects the
/// curve ordering, and the two entity controls are the inclusion toggles.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct RetrieveRequest {
    /// Bank isolation key (required; empty fails closed).
    pub bank: String,
    /// Query text (required; also embedded for the dense leg).
    pub query: String,
    /// Domain filters (`[]` = no filter).
    pub domains: Vec<String>,
    /// Final result count.
    pub limit: usize,
    /// Point-in-time filter (bi-temporal).
    pub as_of: Option<String>,
    /// Axis for `as_of` (default valid).
    pub time_axis: TimeAxis,
    /// Bounded graph expansion on seeds.
    pub expand_graph: bool,
    /// Optional pack budget applied to the returned hits. Trims trailing hits;
    /// the first hit is always returned even if it alone exceeds the budget
    /// (compose re-truncates composed text on injection).
    pub budget_tokens: Option<usize>,
    /// Injectable clock for deterministic recency boosts (defaults to system UTC).
    pub now: Option<String>,
    /// Explicit anchor for a relative temporal expression in the query:
    /// RFC 3339 text with an explicit UTC offset (for example
    /// `2026-09-26T10:00:00Z`). `None` means the temporal arm never runs, even
    /// when the query holds a supported expression: there is no server-time
    /// fallback. A malformed value is rejected as `invalid_argument`.
    pub query_anchor: Option<String>,
    /// Recall scope preset, already resolved against the bank default.
    pub recall_scope: RecallScope,
    /// Drop raw items whose consolidated parent is also in the candidate set.
    pub prefer_consolidated: bool,
    /// Per-bank read-time tolerance level for the near-duplicate cap.
    pub duplicate_tolerance_read: DuplicateTolerance,
    /// Read-time retention curve active flag (per-bank policy). False keeps the
    /// pre-curve order, so a request that leaves it false behaves exactly as it
    /// did before the field existed.
    pub retention_curve: bool,
    /// Curve floor in `(0, 1]`, read only when `retention_curve` is true.
    pub retention_floor: f64,
    /// Optional per-stage score floors. The default sets no floor, so a request
    /// that leaves it empty behaves exactly as it did before the field existed.
    pub min_scores: MinScores,
    /// Provenance floor for this request, already resolved against the bank
    /// default. The rung is an upper bound: `verified` keeps facts only and
    /// `inferred` keeps every class. `None` means no floor, so a request that
    /// names none behaves exactly as it did before the field existed.
    ///
    /// The filter is a candidate prune at fetch time, beside the domain, `as_of`,
    /// and archive filters, so it removes an item from the returned page, the
    /// dedup scope, graph inclusion, and the reinforced set in one step. It never
    /// rewrites an item, so a surviving hit keeps its own `epistemic_kind`,
    /// `source_type`, and `confidence`.
    pub provenance_floor: Option<ProvenanceFloorDefault>,
    /// Publish each hit's `entities[]` names (default true). When false every
    /// hit's `entities` serializes as `[]` and no entity summary is built, so
    /// no entity name is returned anywhere in the response.
    pub include_entities: bool,
    /// Request the top-level entity summary. Requires `include_entities`
    /// (a summary is made of names); the combination is rejected. The summary
    /// is assembled from the returned page's own `entities[]` and never
    /// reaches the warnings, the explanation trace, telemetry, or health.
    pub entity_summary: bool,
    /// Attach each hit's bounded `exact_render` rendering (default false).
    /// When false no rendering is produced and the field is absent on every hit,
    /// so a request that leaves it false behaves exactly as it did before the
    /// field existed.
    ///
    /// When true each hit that carries a snapshot also carries the rendering,
    /// whole or not at all: a snapshot whose rendering exceeds the published cap
    /// yields no rendering rather than a shortened one.
    pub include_exact: bool,
}

impl RetrieveRequest {
    /// Minimal request for one bank/query.
    #[must_use]
    pub fn new(bank: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            bank: bank.into(),
            query: query.into(),
            domains: Vec::new(),
            limit: DEFAULT_LIMIT,
            as_of: None,
            time_axis: TimeAxis::Valid,
            expand_graph: false,
            budget_tokens: None,
            now: None,
            query_anchor: None,
            recall_scope: RecallScope::Full,
            prefer_consolidated: false,
            duplicate_tolerance_read: DuplicateTolerance::Balanced,
            retention_curve: false,
            retention_floor: 0.25,
            min_scores: MinScores::default(),
            provenance_floor: None,
            include_entities: true,
            entity_summary: false,
            include_exact: false,
        }
    }
}

/// One retrieval hit: belief/fact fields plus rank/score metadata.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoredHit {
    /// Frozen fact/belief read contract.
    #[serde(flatten)]
    pub hit: RetrieveHit,
    /// Fused score, relative and never calibrated.
    ///
    /// This is the **pre-rerank** rank-fusion value. When a reranker is
    /// configured it decides the order of the page but does not rewrite this
    /// field, so a page can list hits in an order this number does not
    /// reproduce. The reranker's own value is in [`HitScores::reranker`].
    /// A caller that sorts or thresholds on `score` is therefore not sorting
    /// or thresholding on the order the page is displayed in (#38).
    pub score: f64,
    /// Per-stage scores: `final` mirrors `score`; `reranker` is the provider's
    /// normalized relevance, `null` when rerank did not score the hit;
    /// `semantic`/`keyword` are the producing arms' raw values, `null` when the
    /// arm did not score the hit; and `age_factor` is the retention factor
    /// applied to `final` at the shaping site. The `stage_score` module owns
    /// this field contract.
    pub scores: HitScores,
    /// 1-based dense rank when the item appeared in the dense leg.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dense_rank: Option<usize>,
    /// 1-based lexical rank when the item appeared in the lexical leg.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lexical_rank: Option<usize>,
    /// 1-based temporal-arm rank when the item appeared inside the resolved
    /// relative window. Omitted (rather than `null`) when the arm did not run
    /// or did not return this item, so a non-temporal call serializes exactly
    /// the shape it had before the arm existed. Rank-only provenance: the arm
    /// carries no score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_rank: Option<usize>,
    /// Item id carrying the authoritative snapshot, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<String>,
    /// Entity names taken from the item's snapshot `entity` value, with
    /// trim-only canonicalization. On the span-verifying write paths
    /// (`store`/`batch`) the value was verified against the source text at
    /// write time; a bundle-imported or operator-corrected item carries the
    /// snapshot as supplied, so its name was not span-verified.
    ///
    /// Empty for every hit kind whose snapshot carries no `entity` (triple
    /// carriers, hub-distilled hubs, beliefs, snapshot-less items, non-default
    /// field lists). The field is always serialized, so a consumer sees `[]`
    /// rather than a missing key. These are content-derived names: they are
    /// descriptive hit metadata, never a truth claim, never a ranking signal,
    /// and this field is never projected into the `warnings` list, the
    /// explanation trace, telemetry events, or the health surfaces.
    #[serde(default)]
    pub entities: Vec<String>,
    /// True when the query text mentions one of this hit's `entities[]` names
    /// under the documented case-insensitive containment rule (`entity_match`).
    ///
    /// Derived at retrieval time and never verified against a source span, so
    /// it is non-authoritative: consumers must present it as a derived reason.
    /// It never affects ordering, scores, filtering, or budgets, and it is
    /// omitted from the payload when false (the `consolidated` convention), so
    /// a non-matching hit serializes exactly the shape it had before this
    /// field existed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub entity_match: bool,
    /// True when this hit is an eligible consolidated unit.
    #[serde(default, skip_serializing_if = "is_false")]
    pub consolidated: bool,
    /// Source item ids this consolidated unit claims (empty for raw items).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_ids: Vec<String>,
}

/// Serde helper: omit a `false` boolean so default hit payloads are unchanged.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Retrieval result with PII-safe counts and diagnostics.
#[derive(Debug, Clone, Serialize)]
pub struct RetrieveOutcome {
    /// Ordered hits.
    pub hits: Vec<ScoredHit>,
    /// Candidates kept from the dense leg after filtering, including the
    /// provenance floor, so a stricter floor reads lower.
    pub dense_candidates: usize,
    /// Candidates kept from the lexical leg after filtering, including the
    /// provenance floor, so a stricter floor reads lower.
    pub lexical_candidates: usize,
    /// Candidates the temporal arm returned that survived item fetching.
    /// Present only when the arm ran (a supported expression resolved against
    /// an explicit anchor), so a non-temporal call serializes exactly the keys
    /// it did before this field existed. `Some(0)` means the arm ran and found
    /// no in-window candidate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_candidates: Option<usize>,
    /// Fused candidates that survived filtering, including the provenance
    /// floor, before the final limit. The floor prunes at fetch time, so this
    /// counts the set the floor left, not the set it started from, and a
    /// stricter floor reads lower.
    pub fused_candidates: usize,
    /// Whether the reranker reordered the shortlist.
    pub reranked: bool,
    /// Graph-expanded candidates that entered the fused result. Counted at the
    /// graph-expansion stage, so a later `reranker` or `final` floor can return
    /// fewer hits than this number.
    pub graph_expanded: usize,
    /// Co-activation pairs reinforced on the returned hit set (PII-safe count).
    pub pairs_reinforced: usize,
    /// Estimated tokens of the returned hits' lexical source text (the
    /// retrieve-budget basis). `compose` budgets the composed pack text
    /// (gist, else snapshot ref, else item id) separately, so the two
    /// numbers differ by design while both stay bounded.
    pub estimated_tokens: usize,
    /// Per-stage latency breakdown (PII-safe).
    pub metrics: RetrieveMetrics,
    /// Structured, PII-safe warnings (degraded legs, rerank fallback).
    pub warnings: Vec<String>,
    /// Scope/dedup counters and id lists; omitted when nothing was narrowed.
    #[serde(skip_serializing_if = "DedupReport::is_inert")]
    pub dedup: DedupReport,
    /// Entity names present on the returned page, each mapped to the number of
    /// returned hits carrying it (name-ascending key order). Present only when
    /// the request asked for it, so a request that does not ask serializes
    /// exactly the keys it did before this field existed.
    ///
    /// Entity names are content: this object belongs on the authorized read
    /// payload only and is never projected into `warnings`, the explanation
    /// trace, telemetry events, or the health surfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_summary: Option<EntitySummary>,
}

/// Per-stage retrieval latency in milliseconds (time only; never content).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RetrieveMetrics {
    /// Dense leg (embed + KNN) duration.
    pub dense_ms: u64,
    /// Lexical leg duration.
    pub lexical_ms: u64,
    /// Graph-expansion stage duration (0 when disabled).
    pub expand_ms: u64,
    /// Rerank stage duration (0 when disabled).
    pub rerank_ms: u64,
    /// Whole-request duration.
    pub total_ms: u64,
}

#[cfg(test)]
#[path = "types_retention_tests.rs"]
mod types_retention_tests;
