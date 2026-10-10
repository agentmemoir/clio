// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one shared projection that turns a
//! retrieval hit into the `explain` trace's per-hit shape.
//!
//! ## Owns
//! - `ExplainHit`, the serialized trace hit view
//! - `TRACE_HIT_KEYS`: the trace-visible hit keys (ids, scores, ranks, counts)
//! - `CONTENT_EXCLUDED_HIT_KEYS` and `SAFE_EXCLUDED_HIT_KEYS`: the deliberate
//!   exclusion tables for every other serialized hit key
//! - `explain_hit`, the exhaustive hit-to-trace mapping
//! - The lossless-read regression module it hosts
//!   (`read_explain_full_record_tests`), which pins that a stored snapshot is
//!   read whole over MCP
//!
//! ## Does not own
//! - The trace's top-level counters and timings object (assembled in
//!   `read_retrieve`)
//! - The frozen `RetrieveHit` / `ScoredHit` contracts (`clio-types`,
//!   `clio-retrieve`)
//! - The compose trace (`read_retrieve::compose_explanation`)
//!
//! ## Boundary
//!
//! Changes to what the explain trace shows per hit should normally require
//! changes here. Changes to retrieval ranking or to the read payload fields
//! should not require changes here.
//!
//! Keep this module focused on the content-free explain hit projection.
//!
//! # Field classification
//!
//! Every serialized hit key MUST sit in exactly one of three sets:
//!
//! - `TRACE_HIT_KEYS` — emitted in the trace. Only PII-safe ids, scores,
//!   ranks, and counts belong here.
//! - `CONTENT_EXCLUDED_HIT_KEYS` — content-derived (gist text, source
//!   context, entity names, bounded snapshot renderings). These MUST NOT enter
//!   the trace; the boundary sweep in `read_explain_tests` proves the trace
//!   carries none of them.
//! - `SAFE_EXCLUDED_HIT_KEYS` — PII-safe but deliberately absent from the
//!   trace today. Moving one into `TRACE_HIT_KEYS` is a deliberate trace
//!   change (for example, adding `scores` later), never an accident.
//!
//! Adding a field to `ScoredHit` breaks compilation in `explain_hit` (the
//! destructuring pattern is exhaustive) and fails the drift guard test until
//! the field is classified in this module. That is the intended forcing
//! function: do not paper over it with `..` or a wildcard. The two mechanisms
//! enforce different things: the exhaustive pattern only forces every field
//! to be mentioned (`newfield: _` compiles), while the drift-guard sweep in
//! `read_explain_guard_tests` forces every field name to be classified. That
//! sweep walks the struct fields by name, not the serialized keys, so a
//! field serde skips cannot hide from it. Reintroducing `..` in `explain_hit`
//! cannot silently drop a trace-visible field either: the guard also asserts
//! the projection emits exactly the allowlist.
//!
//! The three tables below are the shipped classification contract, not a
//! test artifact: the projection, the wire contract tests, and the drift
//! guard all read the same constants.

use clio_retrieve::ScoredHit;
use serde::Serialize;

/// Per-hit keys the explain trace emits.
///
/// The key order is not part of the contract: the trace is a JSON object and
/// neither the projection nor any assertion depends on key order.
pub const TRACE_HIT_KEYS: [&str; 6] = [
    "item_id",
    "score",
    "dense_rank",
    "lexical_rank",
    "consolidated",
    "source_ids",
];

/// Content-derived hit keys that MUST NOT enter the explain trace.
///
/// `exact_render` is a bounded rendering of an item's stored snapshot, so it is
/// content the same way `gist` and `context` are: it holds the snapshot's own
/// values, and a trace that carried it would carry the snapshot. Its size cap
/// bounds the cost, never the sensitivity. It stays here rather than in
/// `TRACE_HIT_KEYS` whatever cap it carries, and the caller who wants the
/// exact stored value reads `get_snapshot` instead.
pub const CONTENT_EXCLUDED_HIT_KEYS: [&str; 4] = ["gist", "context", "entities", "exact_render"];

/// PII-safe hit keys deliberately not in the trace today.
///
/// `temporal_rank` is the temporal arm's payload rank: it is already visible
/// on the read payload's hits and through the trace's conditional top-level
/// `temporal_candidates` counter, so the per-hit trace keeps its frozen shape
/// without it.
///
/// `scores` is the whole nested score object, so `scores.trust_factor` and
/// every other nested key are covered by that one entry and are not classified
/// separately.
pub const SAFE_EXCLUDED_HIT_KEYS: [&str; 12] = [
    "bank_id",
    "kind",
    "category",
    "episodic_type",
    "epistemic_kind",
    "source_type",
    "confidence",
    "requires_confidence_check",
    "scores",
    "snapshot_ref",
    "entity_match",
    "temporal_rank",
];

/// The explain trace's per-hit view: ids, score, ranks, and counts only.
///
/// No key is skip-serialized, so every hit keeps the same wire shape and a
/// missing value is `null` (or an empty array), never a missing key.
#[derive(Debug, Serialize)]
pub(crate) struct ExplainHit<'a> {
    /// Item or belief id.
    pub(crate) item_id: &'a str,
    /// Fused (and possibly boosted or reranked) score.
    pub(crate) score: f64,
    /// 1-based dense rank; `null` when the dense leg did not return the hit.
    pub(crate) dense_rank: Option<usize>,
    /// 1-based lexical rank; `null` when the lexical leg did not return the hit.
    pub(crate) lexical_rank: Option<usize>,
    /// True when this hit is an eligible consolidated unit.
    pub(crate) consolidated: bool,
    /// Source item ids a consolidated unit claims (empty for raw items).
    pub(crate) source_ids: &'a [String],
}

/// Project one hit into the trace view.
///
/// The destructuring pattern is exhaustive by design: adding a field to
/// `ScoredHit` fails compilation here until the field is deliberately
/// classified in the constants above.
#[must_use]
pub(crate) fn explain_hit(hit: &ScoredHit) -> ExplainHit<'_> {
    let ScoredHit {
        hit: contract,
        score,
        scores: _,
        dense_rank,
        lexical_rank,
        temporal_rank: _,
        snapshot_ref: _,
        entities: _,
        entity_match: _,
        consolidated,
        source_ids,
    } = hit;
    ExplainHit {
        item_id: &contract.item_id,
        score: *score,
        dense_rank: *dense_rank,
        lexical_rank: *lexical_rank,
        consolidated: *consolidated,
        source_ids: source_ids.as_slice(),
    }
}

#[cfg(test)]
#[path = "read_explain_tests.rs"]
mod read_explain_tests;

#[cfg(test)]
#[path = "read_explain_guard_tests.rs"]
mod read_explain_guard_tests;

#[cfg(test)]
#[path = "read_explain_full_record_tests.rs"]
mod read_explain_full_record_tests;
