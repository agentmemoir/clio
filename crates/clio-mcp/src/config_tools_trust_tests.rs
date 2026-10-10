// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the trust-reach tests of the MCP config
//! surface: the live-retrieval fixture over the fixture bank and the tests
//! proving `config_profile_apply` reaches the live retriever on success and
//! leaves it untouched on failure.
//!
//! ## Owns
//! - The live-retrieval fixture: `retrieval_state`, the agent-inferred belief
//!   builder, the store/lexical `seed` helper, and the `set_agent_inferred`
//!   dispatcher helper
//! - `trust_factor_bits`, the bit-pattern probe through the MCP `retrieve`
//!   tool
//! - The successful-apply and failed-apply liveness tests over the live
//!   retriever
//!
//! ## Does not own
//! - The shared dispatcher helpers `call`, `ok`, and `code_of`
//!   (`config_tools_tests`)
//! - The rest of the config/ranking surface tests (`config_tools_tests`)
//! - The trust multiplier mapping and its computation (`clio-retrieve`
//!   `hybrid_rank::trust_factor_for`)
//!
//! ## Boundary
//!
//! Changes to how a config/ranking mutation reaches the live retriever should
//! normally require changes here. Changes to the config semantics should not.
//!
//! Keep this module focused on the `config_profile_apply` trust-reach tests.

use super::config_tools_tests::{call, code_of, ok};
use crate::runtime::{McpOpenOptions, McpState};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory, SourceType};
use serde_json::Value;
use std::sync::Arc;

/// The bank the live-retrieval fixtures use.
const BANK: &str = "bank-a";

/// A hermetic state over the fixture bank: no embedder resolves, so the
/// retrieval runs the lexical leg and the trust shaping.
fn retrieval_state() -> Arc<McpState> {
    let opts = McpOpenOptions {
        default_bank: BANK.into(),
        ..McpOpenOptions::default()
    };
    McpState::open_with_effective(&opts, &serde_json::json!({})).expect("state")
}

/// An agent-inferred belief, the provenance class the shipping `0.85` default
/// demotes. Field shape from `read_scores_test_support::base_item`, private to
/// the read tests.
fn agent_inferred_belief(id: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: id.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Belief,
        source_type: Some(SourceType::AgentInferred),
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn".into()),
        snapshot: Some(serde_json::json!({"entity": gist})),
        gist: Some(gist.into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Seed one item through the state's own store and lexical index.
fn seed(state: &McpState, item: &MemoryItem, text: &str) {
    state
        .store
        .create_memory_item(item, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&item.bank_id, &item.id, text)
        .expect("lexical doc");
}

/// The bit pattern of the seeded hit's `trust_factor` through the MCP
/// `retrieve` tool, the one shared retriever the config tools feed.
fn trust_factor_bits(state: &McpState, query: &str) -> u64 {
    let out = call(
        state,
        "retrieve",
        serde_json::json!({"bank": BANK, "query": query}),
    );
    assert_eq!(out["ok"], serde_json::json!(true), "{out}");
    let hits = out["hits"].as_array().expect("hits array");
    assert!(!hits.is_empty(), "the seeded item matched: {out}");
    hits[0]["scores"]["trust_factor"]
        .as_f64()
        .unwrap_or_else(|| panic!("trust_factor is a number: {out}"))
        .to_bits()
}

/// Set the agent-inferred trust ratio through the real dispatcher, mirroring
/// the `set_ranking` helper in `runtime_ranking_tests`.
fn set_agent_inferred(state: &McpState, ratio: f64) -> Value {
    let patch = serde_json::json!({
        "trust": {"user_stated": 1.0, "third_party": 0.95, "agent_inferred": ratio}
    });
    call(
        state,
        "ranking_env_set",
        serde_json::json!({"patch": patch}),
    )
}

/// A successful `config_profile_apply` pushes the profile's ranking
/// environment into the live retriever: after a session trust change, the
/// apply restores the profile's `0.85` agent-inferred ratio, so the config
/// view and the next retrieval agree.
#[test]
fn a_successful_profile_apply_reaches_the_live_retriever() {
    let state = retrieval_state();
    let belief = agent_inferred_belief("belief", "profile apply liveness marker");
    seed(&state, &belief, "profile apply liveness marker");
    let query = "profile apply liveness marker";
    assert_eq!(
        trust_factor_bits(&state, query),
        0.85f64.to_bits(),
        "the shipping default applies before any change"
    );

    let set = set_agent_inferred(&state, 0.5);
    assert!(ok(&set), "{set}");
    assert_eq!(
        trust_factor_bits(&state, query),
        0.5f64.to_bits(),
        "the session trust change governs the next retrieval"
    );

    let apply_args = serde_json::json!({"name": "coding_local"});
    let apply = call(&state, "config_profile_apply", apply_args);
    assert!(ok(&apply), "{apply}");
    assert_eq!(
        trust_factor_bits(&state, query),
        0.85f64.to_bits(),
        "the profile's system-default ranking must reach the live retriever"
    );
}

/// A failed `config_profile_apply` applies nothing: the retriever keeps the
/// live environment an earlier successful `ranking_env_set` pushed.
#[test]
fn a_failed_profile_apply_leaves_the_live_retriever_alone() {
    let state = retrieval_state();
    let belief = agent_inferred_belief("belief", "failed apply liveness marker");
    seed(&state, &belief, "failed apply liveness marker");
    let query = "failed apply liveness marker";

    let set = set_agent_inferred(&state, 0.5);
    assert!(ok(&set), "{set}");
    assert_eq!(
        trust_factor_bits(&state, query),
        0.5f64.to_bits(),
        "the session trust change governs the next retrieval"
    );

    let failed_args = serde_json::json!({"name": "no-such-profile"});
    let failed = call(&state, "config_profile_apply", failed_args);
    assert!(!ok(&failed), "{failed}");
    assert_eq!(code_of(&failed), "unknown_profile", "{failed}");
    assert_eq!(
        trust_factor_bits(&state, query),
        0.5f64.to_bits(),
        "a failed apply must not touch the retriever's live environment"
    );
}
