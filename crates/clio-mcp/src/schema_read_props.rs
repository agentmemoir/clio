// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the schema property blocks of the read tools:
//! one helper per published read control, each holding the whole published
//! contract of that control as one readable block.
//!
//! ## Owns
//! - `min_scores_prop` and `provenance_floor_prop`: the two read filters, with
//!   the per-name consequences their descriptions state
//! - `include_entities_prop` and `entity_summary_prop`: the two entity toggles
//!   and their documented defaults
//! - `include_exact_prop` and `fidelity_prop`: the bounded-rendering request
//!   control and the pack channel that selects it
//!
//! ## Does not own
//! - The read tool rows that reference these properties (`schema_read_defs`)
//! - Whether a hit's rendering fits its cap (`clio-retrieve` `render`), and
//!   which form a composed hit takes (`clio-retrieve` `pack_forms`)
//! - The floor validation rules and the entity-summary rules (`clio-retrieve`)
//!   ; this module only publishes their contract
//!
//! ## Boundary
//!
//! Changes to a published read control's contract should normally require
//! changes here. Changes to pack assembly or to write tools should not require
//! changes here.
//!
//! Keep this module focused on read-tool schema property blocks.

#[cfg(test)]
#[path = "schema_read_props_tests.rs"]
mod schema_read_props_tests;

use crate::schema_defs::{b, en, num};
use clio_config::retention_levels::PackFidelityDefault;
use clio_retrieve::{EXACT_RENDER_MAX_BYTES, MAX_FLOOR};
use serde_json::{Value, json};

/// The `retrieve` `min_scores` object: four optional, inclusive score floors.
///
/// A private helper so the whole published contract reads as one block instead
/// of being split across the `read_defs` table; see [`provenance_floor_prop`].
pub(crate) fn min_scores_prop() -> Value {
    json!({
        "type": "object",
        "description": "Optional inclusive floors; unknown names refused. Semantic and keyword are arm-local; reranker and final filter hits.",
        "properties": {
            "semantic": num("Floor on the dense arm's cosine similarity, the same scale as scores.semantic, 0..=1. Prunes the dense arm only; a value above 1 is rejected."),
            "keyword": num(&format!("Floor on the lexical arm's raw backend score, the same scale as scores.keyword. Backend-dependent: SQLite FTS5 -bm25 and Postgres ts_rank_cd are different scales, so no cross-backend parity is claimed and one cutoff cannot be carried across backends. Must be <= {MAX_FLOOR}.")),
            "reranker": num("Floor on the reranker's normalized relevance, the same scale as scores.reranker, 0..=1. Applied after ranking, so every returned hit clears it; a candidate the reranker did not score is kept. A value above 1 is rejected."),
            "final": num(&format!("Floor on the fused score of the returned page, the same scale as scores.final. Applied to the returned set after the limit and the token budget, so it never backfills a freed slot. The floor applies to the demoted value — the fused score after `scores.age_factor` and then `scores.trust_factor` have lowered it, in that order — so a caller can predict what a floor excludes from the reported `scores.final`. Must be <= {MAX_FLOOR}."))
        },
        "additionalProperties": false
    })
}

/// The `retrieve` provenance floor: the weakest rung the answer set may hold.
///
/// A private helper for the same reason as [`min_scores_prop`]: the published
/// contract reads as one block instead of being split across the `read_defs`
/// table. The four rungs are the enum, and the ladder reads from the strictest
/// rung (`verified`) to the loosest (`inferred`). The floor decides membership
/// only: an item below the rung is absent from the answer set and from the
/// co-activation reinforcement set, and no stored field of a surviving hit
/// moves. The property stays out of `required`, so the description carries the
/// omitted meaning instead.
pub(crate) fn provenance_floor_prop() -> Value {
    en(
        "Weakest provenance rung kept; hits below it are removed, not ranked lower. Omitted uses the bank default.",
        &["verified", "stated", "third_party", "inferred"],
    )
}

/// The `retrieve` `include_entities` toggle (default true).
pub(crate) fn include_entities_prop() -> Value {
    b(
        "Publish each hit's `entities[]` names (default true). When false, every hit's `entities` serializes as an empty array, the derived `entity_match` flag is absent, and no entity summary is built, so no entity name appears anywhere in the response. Entity names are content: they never reach `warnings`, the `explain` trace, telemetry, or the health surfaces.",
    )
}

/// The `retrieve` `entity_summary` toggle (default false).
///
/// The summary is a roll-up of the returned page's own `entities[]` and maps
/// each name on that page to the number of returned hits carrying it.
pub(crate) fn entity_summary_prop() -> Value {
    b(
        "Return a top-level `entity_summary` object mapping each entity name present on the returned page to the number of returned hits carrying it, with name-ascending keys (default false, so the key is absent unless requested). It is counted from the returned hits' own `entities[]` and never involves new extraction. Rejected together with `include_entities: false`, because a summary is made of names. Like `hits[].entities`, it never reaches `warnings`, the `explain` trace, telemetry, or the health surfaces.",
    )
}

/// The `retrieve` `include_exact` toggle (default false).
///
/// The published cap and the whole-record read both live elsewhere; this
/// property states the policy a caller must know to use the field safely.
pub(crate) fn include_exact_prop() -> Value {
    b(&format!(
        "Attach hits[].exact_render, a bounded snapshot (default false, max {EXACT_RENDER_MAX_BYTES} bytes, never truncated); see get_snapshot."
    ))
}

/// The `compose_context` `fidelity` channel (default from the bank).
///
/// The enum is the profile type itself, so the wire names cannot drift from the
/// bank default that resolves them.
pub(crate) fn fidelity_prop() -> Value {
    en(
        "Composed hit channel: flow (default) reads gist before rendering, exact the reverse. Omitted uses the bank default.",
        &[
            PackFidelityDefault::Flow.as_str(),
            PackFidelityDefault::Exact.as_str(),
        ],
    )
}
