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
        "description": "Optional per-stage score floors. Every floor is inclusive (>=) and an absent or null object imposes no floor at all. Only the four names below are accepted: an unrecognised name is rejected as invalid_argument rather than read as no floor, so a mistyped floor can never return an unfiltered page. `semantic` and `keyword` are arm-local: each prunes only the arm it names, before fusion, so the other arm's candidates are unaffected and a survivor of the other arm reports null for the pruned score. `reranker` and `final` are post-ranking filters, so every returned hit clears them. A floor only removes candidates: it never widens the scope or exceeds the token budget. A floor that prunes before the limit promises no page length: pruning renumbers the survivors, and the limit and the token budget are applied afterwards, so the room a removed hit leaves can go to another hit. Every scale here is relative and never calibrated, so calibrate a cutoff against the observed scores of the same query first; a fixed cutoff can silently drop good results.",
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
        "Weakest provenance rung the returned hits may sit at: verified (strictest, keeps facts only), stated (also keeps user_stated beliefs), third_party (also keeps third-party beliefs), inferred (loosest, keeps every rung). The ladder runs from the strictest rung to the loosest, and a hit below the floor is removed from the answer set rather than ranked lower: it is absent from the result and from co-activation reinforcement, so the floor changes set membership and nothing else — no stored field of a surviving hit moves, and no item of an excluded class is ever returned. A stricter floor promises no page length: the limit, the token budget and duplicate suppression all run after the prune, and each of them can take the room a removed hit leaves, so a floored page need not match an unfloored one in length or in ids. `dense_candidates`, `lexical_candidates` and `fused_candidates` count the candidates that survived the floor, so they read lower with a stricter floor. Omit it and the bank's own provenance_floor_default applies, and a bank that states no default means no floor. Only the four names below are accepted: any other value is rejected as invalid_argument rather than read as no floor, so a mistyped floor can never return an unfiltered page.",
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
        "Attach a bounded rendering of each hit's authoritative half as `hits[].exact_render` (default false, so the key is absent from every hit unless requested). The rendering is the hit's stored snapshot serialized whole, which makes it content: like `gist`, it never reaches `warnings`, the `explain` trace, telemetry, or the health surfaces. It is bounded and never truncated: a snapshot whose rendering would exceed {EXACT_RENDER_MAX_BYTES} bytes yields no rendering for that hit at all, so the field is never a prefix of a snapshot that reads as the stored value. Its token cost is an upper bound rather than an equality: a rendering of at most that many bytes is at most {} tokens under the same heuristic `budget_tokens` uses, which counts characters rather than bytes, so a rendering within three characters of that byte count reaches that figure even when the text holds multi-byte characters, and only a rendering of fewer characters lands below it. A request that omits the toggle produces no rendering, so nothing changes for a caller who does not ask. It is a rendering and not the stored half: read `get_snapshot` for the exact value, because this field never replaces that read.",
        EXACT_RENDER_MAX_BYTES / 4
    ))
}

/// The `compose_context` `fidelity` channel (default from the bank).
///
/// The enum is the profile type itself, so the wire names cannot drift from the
/// bank default that resolves them.
pub(crate) fn fidelity_prop() -> Value {
    en(
        &format!(
            "Which half a composed hit contributes: flow (default) packs the narrative channel, reading each hit's gist first and its bounded rendering second, and exact reads the bounded rendering first and the gist second. Only the two names below are accepted: any other value is rejected as invalid_argument rather than packed as flow, so a mistyped channel is never silently downgraded. Omit it and the bank's own pack_fidelity_default applies, and a bank that states none packs flow. A rendering is charged against the same budget_tokens as every other packed text: one that does not fit the remaining budget is dropped whole for that hit rather than shortened, and a rendering is separately bounded at {EXACT_RENDER_MAX_BYTES} bytes, so a snapshot over that byte bound contributes no rendering at all. A hit whose rendering is absent contributes its gist instead; a hit with neither a rendering nor a gist is marked as a reference rather than a bare identifier, and so is a hit whose rendering did not fit the remaining budget, so the packed text and the pack's per-form tally cannot disagree."
        ),
        &[
            PackFidelityDefault::Flow.as_str(),
            PackFidelityDefault::Exact.as_str(),
        ],
    )
}
