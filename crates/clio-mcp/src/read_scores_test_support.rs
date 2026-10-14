// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the state and payload helpers shared by the three
//! `read_scores` test modules: the hermetic store fixture, the dispatcher
//! call, the seeded item, the lexical write, and the payload accessors.
//!
//! ## Owns
//! - `state`, the hermetic empty-effective-config store fixture whose
//!   retention defaults match the builtin default profile
//! - `call`, the MCP dispatcher entry used by both modules
//! - `base_item`, the seeded memory item with a fixed stamp and admission
//!   score
//! - `put`, the direct store and lexical-index write
//! - `hits`, the `retrieve` payload hit accessor
//! - `scores_of`, the documented-key-set accessor used for value assertions
//!
//! ## Does not own
//! - Any test
//! - Module-only helpers: `hit_ids`, `hit_by_id`, and
//!   `assert_final_mirrors_score` stay in `read_scores_tests`, and the fake
//!   TEI double and the durable store args stay with their single users
//! - The code under test (the MCP read handlers and `clio-retrieve` scoring)
//!
//! ## Boundary
//!
//! Changes to what the fixtures contain should normally require changes here.
//! Changes to how a single test asserts should not require changes here.
//!
//! Keep this module focused on the shared `read_scores` fixtures.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// Hermetic empty effective config: no embedder is resolved, so the dense arm
/// is off and `semantic` must be `null` rather than an environment-dependent
/// value. The retention defaults stay `full` / `balanced`, which is what the
/// builtin default profile — the explicit-parameter in-process surface's
/// fallback — applies too.
pub(super) fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: "bank-a".into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
pub(super) fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

pub(super) fn base_item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: Some("turn-1".into()),
        snapshot: Some(json!({"entity": gist})),
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

pub(super) fn put(state: &McpState, item: &MemoryItem, text: &str) {
    state
        .store
        .create_memory_item(item, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&item.bank_id, &item.id, text)
        .expect("lexical doc");
}

pub(super) fn hits(payload: &Value) -> &[Value] {
    payload["hits"].as_array().expect("hits array")
}

/// Exactly the documented key set, returned for value assertions.
pub(super) fn scores_of(hit: &Value) -> &Map<String, Value> {
    let scores = hit["scores"]
        .as_object()
        .unwrap_or_else(|| panic!("scores object on every hit: {hit}"));
    let mut keys: Vec<&str> = scores.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "age_factor",
            "final",
            "keyword",
            "reranker",
            "semantic",
            "trust_factor"
        ],
        "{hit}"
    );
    scores
}
