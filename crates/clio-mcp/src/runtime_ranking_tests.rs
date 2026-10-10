// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests on the live ranking plumbing: that
//! the process's *current* ranking environment reaches the hub engine and the
//! shared retriever, that a lowered co-activation threshold changes hub
//! eligibility in the same process, that a live trust ratio and a live
//! retrieval weight change the next retrieval, and that a fresh process still
//! starts from the effective defaults.
//!
//! ## Owns
//! - `McpState::ranking_policy` and `apply_ranking_policy` assertions
//! - The live trust-ratio and retrieval-weight tests through the real
//!   `ranking_env_set` write path
//!
//! ## Does not own
//! - The `consolidate` response shape (`mutator_tools_tests`)
//! - The hub algorithm and its own policy seam (`clio-write`'s
//!   `hub_distill_tests`)
//! - End-to-end evidence over the real binary (the `clio` consolidate
//!   harness)
//!
//! ## Boundary
//!
//! Keep this module focused on ranking-to-engine plumbing.

use crate::runtime::{McpOpenOptions, McpState};
use clio_config::ranking::{CoactivationConstants, RankingEnv, RankingEnvPatch};
use clio_store::AssocPolicy;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory, SourceType};
use serde_json::{Value, json};
use std::sync::Arc;

/// A state over an isolated in-memory store, the way the runtime opens one.
fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: id.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn".into()),
        snapshot: Some(json!({"k": "v"})),
        gist: Some(format!("gist {id}")),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Milliseconds since the epoch, computed the way the engine's own clock does.
fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// Seed a bank whose top node has co-activation degree 3 through the state's own
/// store, so the graph and the engine under test are the same store.
///
/// Edges are stamped with the current time because a co-activation edge's
/// effective weight decays lazily from its last update. Nothing here asserts on
/// a clock value; a fresh stamp only keeps the seeded weight above the prune
/// threshold, which is the state a real recall leaves behind.
fn seed_degree_three(state: &McpState, bank: &str) {
    let store = &state.store;
    store
        .create_memory_item(&item("hub", bank), "agent")
        .expect("hub item");
    for neighbor in ["n1", "n2", "n3"] {
        store
            .create_memory_item(&item(neighbor, bank), "agent")
            .expect("neighbor item");
        store
            .reinforce_assoc_edge(bank, "hub", neighbor, now_millis(), 0.15)
            .expect("reinforce");
    }
}

/// Patch the process's live ranking environment the way `ranking_env_set` does.
///
/// The co-activation block is a whole struct, so the patch is built from the
/// current constants with one field changed, not from a partial literal.
fn set_hub_degree_limit(state: &McpState, limit: u32) {
    let mut rt = state.config.lock().expect("config runtime mutex poisoned");
    let patch = RankingEnvPatch {
        coactivation: Some(CoactivationConstants {
            hub_degree_limit: limit,
            ..rt.ranking_env().coactivation.clone()
        }),
        ..RankingEnvPatch::default()
    };
    assert!(
        matches!(
            rt.ranking_env_set(&patch, false),
            clio_config::error::ToolResult::Ok { .. }
        ),
        "ranking_env_set should apply"
    );
}

fn eligible(state: &McpState, bank: &str) -> Vec<String> {
    state
        .hub
        .as_ref()
        .expect("hub engine is present")
        .eligible_hubs(bank)
        .expect("hub scan")
}

#[test]
fn ranking_policy_matches_the_single_conversion() {
    let env = RankingEnv::system_defaults();
    assert_eq!(McpState::ranking_policy(&env), AssocPolicy::default());

    let mut tuned = RankingEnv::system_defaults();
    tuned.coactivation = CoactivationConstants {
        hub_degree_limit: 1,
        w_min: 0.11,
        ..CoactivationConstants::default()
    };
    let policy = McpState::ranking_policy(&tuned);
    assert_eq!(policy.hub_degree_limit, 1);
    assert!((policy.w_min - 0.11).abs() < 1e-12);
}

#[test]
fn a_fresh_state_starts_from_the_effective_defaults() {
    let state = state();
    seed_degree_three(&state, "bank");
    // Degree 3 is below the default limit of 16, so a fresh process sees no
    // hub and applying the defaults changes nothing.
    state.apply_ranking_policy();
    let at_defaults = eligible(&state, "bank");
    assert!(at_defaults.is_empty(), "{at_defaults:?}");
}

#[test]
fn a_ranking_change_in_this_process_governs_a_later_wave() {
    let state = state();
    seed_degree_three(&state, "bank");
    set_hub_degree_limit(&state, 1);

    // Pin the defect this phase fixes: the value is set and readable, but the
    // engine still holds the value it was constructed with.
    let before_apply = eligible(&state, "bank");
    assert!(before_apply.is_empty(), "{before_apply:?}");

    state.apply_ranking_policy();
    assert_eq!(eligible(&state, "bank"), vec!["hub".to_owned()]);
}

#[test]
fn a_second_apply_is_idempotent() {
    let state = state();
    seed_degree_three(&state, "bank");
    set_hub_degree_limit(&state, 1);
    state.apply_ranking_policy();
    state.apply_ranking_policy();
    assert_eq!(eligible(&state, "bank"), vec!["hub".to_owned()]);
}

/// The bank and fixed clock every retrieval fixture uses, so the age factor is
/// pinned and no test reads the host clock.
const BANK: &str = "ranking-live";
const FIXED_NOW: &str = "2026-01-02T00:00:00Z";

/// A state over an isolated in-memory store whose config tools serve a
/// hermetic, env-free runtime, so retrieval runs the lexical leg plus the
/// shaping under test and no provider is resolved from the host.
fn retrieval_state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: BANK.into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("state")
}

/// Seed one item through the state's own store and lexical index, so the
/// retrieval under test reads the same data the fixture wrote.
fn seed(state: &McpState, memory: &MemoryItem, text: &str) {
    state
        .store
        .create_memory_item(memory, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&memory.bank_id, &memory.id, text)
        .expect("lexical doc");
}

/// Seed an agent-inferred belief: the class the shipping trust defaults demote.
fn seed_agent_inferred_belief(state: &McpState, id: &str, text: &str) {
    let mut belief = item(id, BANK);
    belief.epistemic_kind = EpistemicKind::Belief;
    belief.source_type = Some(SourceType::AgentInferred);
    seed(state, &belief, text);
}

/// One retrieval through the engine's JSON surface with the clock pinned.
///
/// The MCP `retrieve` binding declares no timestamp argument, so this is the
/// seam that supports a fixed `now`; the request still goes through the one
/// shared retriever, which is what the live ranking plumbing feeds.
fn retrieve(state: &McpState, query: &str) -> Value {
    let outcome = serde_json::to_value(clio_retrieve::retrieve_from_json(
        &state.retriever,
        &json!({"bank": BANK, "query": query, "now": FIXED_NOW}),
    ))
    .expect("serialize retrieve outcome");
    assert_eq!(outcome["ok"], json!(true), "{outcome}");
    outcome
}

/// The `scores` object of the one seeded hit, proving the hit was returned.
fn scores_for<'a>(payload: &'a Value, id: &str) -> &'a serde_json::Map<String, Value> {
    payload["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .find(|hit| hit["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))["scores"]
        .as_object()
        .expect("scores object")
}

/// One `scores` number; a missing or null value is a fixture failure.
fn scores_f64(scores: &serde_json::Map<String, Value>, key: &str) -> f64 {
    scores[key]
        .as_f64()
        .unwrap_or_else(|| panic!("`{key}` is a number: {scores:?}"))
}

/// Apply one ranking patch through the real write dispatcher, the same path a
/// `ranking_env_set` MCP call takes.
fn set_ranking(state: &McpState, patch: &Value, dry_run: bool) -> Value {
    crate::write_tools::dispatch(
        state,
        "ranking_env_set",
        &json!({"patch": patch, "dry_run": dry_run}),
    )
}

/// A live `ranking_env_set` changes the trust ratio the *next* retrieval
/// applies to an agent-inferred belief, and the returned hit reports the
/// applied value.
#[test]
fn a_live_trust_ratio_governs_the_next_retrieval() {
    let state = retrieval_state();
    seed_agent_inferred_belief(&state, "belief", "trust liveness marker");
    let query = "trust liveness marker";

    let before = retrieve(&state, query);
    assert_eq!(
        scores_f64(scores_for(&before, "belief"), "trust_factor").to_bits(),
        0.85f64.to_bits(),
        "the shipping default applies before the set: {before}"
    );

    let set = set_ranking(
        &state,
        &json!({"trust": {"user_stated": 1.0, "third_party": 0.95, "agent_inferred": 0.5}}),
        false,
    );
    assert_eq!(set["ok"], json!(true), "{set}");

    let after = retrieve(&state, query);
    assert_eq!(
        scores_f64(scores_for(&after, "belief"), "trust_factor").to_bits(),
        0.5f64.to_bits(),
        "the new ratio governs the next retrieval: {after}"
    );
}

/// A live `ranking_env_set` changes an existing retrieval weight the next
/// retrieval applies: the admission shaping uses the new importance weight
/// even though the retriever object was never rebuilt.
#[test]
fn a_live_importance_weight_governs_the_next_retrieval() {
    let state = retrieval_state();
    let mut fact = item("fact", BANK);
    fact.admission_score = Some(0.6);
    seed(&state, &fact, "importance liveness marker");
    let query = "importance liveness marker";

    let before = scores_f64(scores_for(&retrieve(&state, query), "fact"), "final");

    // 0.5/0.5/3.0 normalizes to importance 0.75 with the dense/lexical ratio
    // unchanged, so the fused score is identical and only the shaping moves.
    let set = set_ranking(
        &state,
        &json!({"retrieval": {"dense": 0.5, "lexical": 0.5, "importance": 3.0}}),
        false,
    );
    assert_eq!(set["ok"], json!(true), "{set}");

    let after = scores_f64(scores_for(&retrieve(&state, query), "fact"), "final");

    let expected_ratio = (1.0 + 0.75 * 0.6) / (1.0 + (1.0 / 3.0) * 0.6);
    let observed_ratio = after / before;
    assert!(
        (observed_ratio - expected_ratio).abs() < 1e-9,
        "importance 0.75 must replace the construction-time 1/3: before={before}, \
         after={after}, ratio={observed_ratio}, expected={expected_ratio}"
    );
}

/// A dry-run `ranking_env_set` reports `applied: false` and leaves the next
/// retrieval on the shipping ratio.
#[test]
fn a_dry_run_ranking_change_leaves_the_next_retrieval_alone() {
    let state = retrieval_state();
    seed_agent_inferred_belief(&state, "belief", "dry run marker");
    let query = "dry run marker";

    let dry = set_ranking(
        &state,
        &json!({"trust": {"user_stated": 1.0, "third_party": 0.95, "agent_inferred": 0.5}}),
        true,
    );
    assert_eq!(dry["ok"], json!(true), "{dry}");
    assert_eq!(dry["applied"], json!(false), "{dry}");

    let after = retrieve(&state, query);
    assert_eq!(
        scores_f64(scores_for(&after, "belief"), "trust_factor").to_bits(),
        0.85f64.to_bits(),
        "a dry run must not reach the retriever: {after}"
    );
}
