// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Regression coverage for an explicit retrieval floor that removes every hit.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_index::Embedder;
use clio_store::VectorSlot;
use clio_types::AmError;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "floor-warning";

fn state() -> Arc<McpState> {
    let options = McpOpenOptions {
        default_bank: BANK.into(),
        ..McpOpenOptions::default()
    };
    McpState::open_with_effective(&options, &json!({})).expect("state")
}

fn dense_state() -> Arc<McpState> {
    let mut state = state();
    let runtime = Arc::get_mut(&mut state).expect("unique state");
    let space = runtime.store.embedding_space().expect("embedding space");
    Arc::get_mut(&mut runtime.retriever)
        .expect("unique retriever")
        .set_embedder(Arc::new(FixedEmbedder {
            dims: space.dims,
            model_id: space.model_id,
        }));
    state
}

struct FixedEmbedder {
    dims: usize,
    model_id: String,
}

impl Embedder for FixedEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts
            .iter()
            .map(|_| {
                let mut vector = vec![0.0; self.dims];
                vector[0] = 1.0;
                vector
            })
            .collect())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        self.model_id.clone()
    }

    fn dims(&self) -> usize {
        self.dims
    }
}

fn seed(state: &McpState) {
    let item = MemoryItem {
        id: "floor-candidate".into(),
        bank_id: BANK.into(),
        subject_id: "subject".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: None,
        snapshot: Some(json!({"entity": "floor-candidate"})),
        gist: Some("alpha beta candidate".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    };
    state
        .store
        .create_memory_item(&item, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(BANK, &item.id, "alpha beta candidate")
        .expect("index item");
}

#[allow(clippy::needless_pass_by_value)]
fn retrieve(state: &McpState, args: Value) -> Value {
    dispatch(state, "retrieve", &args)
}

#[test]
fn a_floor_that_removes_every_hit_adds_a_warning() {
    let s = state();
    seed(&s);
    let result = retrieve(
        &s,
        json!({
            "query": "alpha beta",
            "min_scores": {"final": 1_000_000.0},
            "explain": true
        }),
    );
    assert_eq!(result["ok"], json!(true), "{result}");
    assert!(
        result["hits"].as_array().is_some_and(Vec::is_empty),
        "{result}"
    );
    for warnings in [&result["warnings"], &result["explanation"]["warnings"]] {
        assert!(
            warnings
                .as_array()
                .is_some_and(|values| values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.contains("min_scores"))
                })),
            "missing floor warning: {result}"
        );
    }
}

#[test]
fn semantic_floor_that_prunes_the_dense_arm_adds_a_warning() {
    let state = dense_state();
    seed(&state);
    state
        .index
        .delete_lexical_doc(BANK, "floor-candidate")
        .expect("remove lexical doc");
    let space = state.store.embedding_space().expect("embedding space");
    let mut vector = vec![0.0; space.dims];
    vector[0] = -1.0;
    state
        .store
        .upsert_vector(&VectorSlot {
            item_id: "floor-candidate".into(),
            bank_id: BANK.into(),
            model_id: space.model_id,
            embedding: vector,
        })
        .expect("vector");

    let result = retrieve(
        &state,
        json!({"query": "unmatched words", "min_scores": {"semantic": 1.0}, "explain": true}),
    );
    assert_eq!(result["ok"], json!(true), "{result}");
    assert_floor_warning(&result);
}

#[test]
fn keyword_floor_that_prunes_the_lexical_arm_adds_a_warning() {
    let state = state();
    seed(&state);
    let result = retrieve(
        &state,
        json!({"query": "alpha beta", "min_scores": {"keyword": 1_000_000.0}, "explain": true}),
    );
    assert_eq!(result["ok"], json!(true), "{result}");
    assert_floor_warning(&result);
}

fn assert_floor_warning(result: &Value) {
    assert!(
        result["hits"].as_array().is_some_and(Vec::is_empty),
        "{result}"
    );
    for warnings in [&result["warnings"], &result["explanation"]["warnings"]] {
        assert!(
            warnings
                .as_array()
                .is_some_and(|values| values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.contains("min_scores"))
                })),
            "missing floor warning: {result}"
        );
    }
}
