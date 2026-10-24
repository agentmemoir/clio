// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-retrieve` crate root: the per-turn intent gate, hybrid
//! dense + lexical retrieval with rank fusion and optional rerank, and the
//! bounded `compose_context` packer.
//!
//! ## Owns
//! - Public module graph and re-exports for gate / retrieve / compose
//! - Fusion, domain/temporal filtering, token accounting, and rerank seams
//!
//! ## Does not own
//! - Backend search SQL (`clio_store::IndexStore`), embeddings (`clio_index`),
//!   admission scoring (`clio-admission`), or persona document CRUD
//! - MCP transport bindings (later work); this crate exposes in-process tools
//!
//! ## Boundary
//!
//! Changes to gate/retrieval/compose behavior should normally require changes
//! here. Changes to backend SQL, embedding transport, or ranking env math should
//! not require changes here.
//!
//! Keep this crate focused on retrieval control-plane and context composition.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::float_cmp))]
#![cfg_attr(test, allow(clippy::cast_possible_truncation))]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::redundant_clone)]

mod assoc;
mod assoc_graph;
mod assoc_surface;
mod compose;
mod dedup;
mod dedup_cap;
mod entities;
mod entity_match;
mod entity_summary;
mod filter;
mod finalize;
mod floors;
mod fusion;
mod hybrid;
mod hybrid_expand;
mod hybrid_fetch;
mod hybrid_legs;
mod hybrid_rank;
mod hybrid_util;
mod intent;
mod live_ranking;
mod pack_forms;
mod render;
mod rerank;
mod rerank_parse;
mod stage_score;
mod surface;
mod surface_policy;
mod temporal;
mod token;
mod types;

pub use assoc::{
    AssocEdgeView, AssociationEngine, DEFAULT_QUERY_NODE_CAP, GraphNodeView, StoreGraphExpander,
};
pub use assoc_surface::{
    AssociationsArgs, GraphLinkArgs, GraphQueryArgs, associations_from_json, graph_link_from_json,
    graph_query_from_json,
};
pub use compose::{
    ComposePack, ComposeRequest, Composer, DEFAULT_BUDGET_TOKENS, DEFAULT_PERSONA_BUDGET_TOKENS,
    PACK_OVERHEAD_TOKENS, PackSection, PersonaEntry,
};
pub use dedup::{
    DedupConfig, DedupReport, HUB_DISTILL_SOURCE_REF, STALE_SOURCE_REMOVED,
    consolidated_source_ids, is_consolidated_item, is_source_removed_stale, is_stale,
};
pub use entity_summary::{EntitySummary, resolve_entity_controls, summarize_entities};
pub use filter::{age_days, item_in_domains, item_visible_at, recency_score, system_now_iso};
pub use floors::{
    FLOOR_BINDING_WARNING_SUFFIX, MAX_FLOOR, MinScores, apply_keyword_floor, apply_reranker_floor,
    apply_semantic_floor, binding_floor_warnings, final_floor_keeps, min_scores_from_value,
    provenance_floor_keeps, provenance_rung,
};
pub use fusion::{
    ARM_DENSE, ARM_LEXICAL, ARM_TEMPORAL, ArmSignal, DEFAULT_RRF_K, DEFAULT_TEMPORAL_ARM_WEIGHT,
    FUSION_ARMS, FusedHit, RankList, StageHit, rrf_fuse,
};
pub use hybrid::HybridRetriever;
pub use intent::{
    DEFAULT_FALSE_NEGATIVE_BOUND, DEFAULT_SKIP_CONFIDENCE, GateDecision, GateEvaluation,
    GateMetrics, GateMetricsSnapshot, evaluate_gate, intent_gate,
};
pub use pack_forms::{FormTally, HitForm, REFERENCE_MARKER};
pub use render::{EXACT_RENDER_MAX_BYTES, render_exact};
pub use rerank::{CohereReranker, HttpReranker, Reranker, build_reranker, build_reranker_from};
pub use rerank_parse::{RerankedDoc, normalize_rerank_score, parse_rerank_response};
pub use stage_score::{HitScores, semantic_from_distance};
pub use surface::{
    ComposeArgs, IntentGateArgs, ReadPolicyResolver, RetrieveArgs, compose_from_json,
    compose_from_json_with_policy, intent_gate_from_json, retrieve_from_json,
    retrieve_from_json_with_policy,
};
pub use temporal::{MAX_TEMPORAL_QUERY_BYTES, TemporalWindow, resolve_window};
pub use token::estimate_tokens;
pub use types::{
    DEFAULT_GRAPH_DECAY, DEFAULT_LEG_FETCH, DEFAULT_LIMIT, DEFAULT_SHORTLIST_M, GraphExpander,
    NoopGraphExpander, RetrieveMetrics, RetrieveOutcome, RetrieveRequest, ScoredHit,
};

#[cfg(test)]
mod fixtures_tests;

#[cfg(test)]
mod rerank_seam;
