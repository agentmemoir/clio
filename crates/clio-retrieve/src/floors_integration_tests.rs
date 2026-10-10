// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the score-floor behavior over the real
//! retrieval path: arm-local pruning before fusion, the post-rerank and
//! post-ranking floors, the no-backfill rule, the default no-op, and the
//! fail-closed validation.
//!
//! ## Owns
//! - `semantic` / `keyword` arm-local floor evidence, including the dense
//!   candidate counter and the `null` arm value of a survivor
//! - `reranker` floor with rerank on, off, and failed (fail-open)
//! - `final` floor on the returned page, budget interaction, and no backfill
//! - The default request's unchanged top-level payload keys
//! - Rejection of an invalid floor and of a contradictory entity control
//!
//! ## Does not own
//! - The floor predicates themselves (`floors_tests`)
//! - Binding-level payloads (`clio-mcp` / `clio` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the arm-local floors over the retrieve path.

use crate::fixtures_tests as fx;
use crate::floors::MinScores;
use crate::types::RetrieveRequest;
use clio_index::{DenseSearchArgs, LexicalSearchArgs, dense_search, lexical_search};
use clio_store::{IndexStore, SqliteStore};
use clio_types::MemoryItem;
use serde_json::Value;
use std::sync::Arc;

/// Request with a frozen clock so recency boosts stay deterministic.
pub(crate) fn request(bank: &str, query: &str) -> RetrieveRequest {
    let mut req = RetrieveRequest::new(bank, query);
    req.now = Some("2026-01-02T00:00:00Z".into());
    req
}

/// Persist one item with a dense vector and a lexical document.
fn put_both(store: &Arc<SqliteStore>, entry: &MemoryItem, text: &str, embedding: Vec<f32>) {
    fx::put_vector(store, entry, embedding);
    store
        .put_lexical_doc(&entry.bank_id, &entry.id, text)
        .expect("put lexical doc");
}

/// Raw cosine distance the dense leg sees for one id.
fn dense_distance(store: &Arc<SqliteStore>, bank: &str, query: &[f32], id: &str) -> f64 {
    dense_search(
        store.as_ref(),
        &DenseSearchArgs {
            bank,
            query,
            domains: Vec::new(),
            limit: 10,
        },
    )
    .expect("dense search")
    .iter()
    .find(|hit| hit.item_id == id)
    .expect("dense hit")
    .distance
}

/// Raw backend lexical score the lexical arm sees for one id.
fn lexical_score(store: &Arc<SqliteStore>, bank: &str, query: &str, id: &str) -> f64 {
    lexical_search(
        store.as_ref(),
        &LexicalSearchArgs {
            bank,
            query,
            domains: Vec::new(),
            limit: 10,
        },
    )
    .expect("lexical search")
    .iter()
    .find(|hit| hit.item_id == id)
    .expect("lexical hit")
    .score
}

/// Ids of the returned page, in rank order.
pub(crate) fn hit_ids(out: &crate::types::RetrieveOutcome) -> Vec<String> {
    out.hits.iter().map(|hit| hit.hit.item_id.clone()).collect()
}

/// The outcome as the wire payload, for key-set and boundary assertions.
pub(crate) fn serialized(out: &crate::types::RetrieveOutcome) -> Value {
    serde_json::to_value(out).expect("serialize outcome")
}

/// The top-level keys a default call emitted before the score floors and the
/// entity controls existed. `dedup` is absent because the default scope/dedup
/// report is inert, and `entity_summary` is absent because it is not requested.
const DEFAULT_TOP_LEVEL_KEYS: [&str; 10] = [
    "dense_candidates",
    "estimated_tokens",
    "fused_candidates",
    "graph_expanded",
    "hits",
    "lexical_candidates",
    "metrics",
    "pairs_reinforced",
    "reranked",
    "warnings",
];

#[test]
fn a_default_call_keeps_its_pre_existing_top_level_keys() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "alpha memory");
    fx::put_lexical(&store, &entry, "alpha memory token");
    let out = fx::retriever(&store)
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");
    assert_eq!(hit_ids(&out), vec!["A".to_owned()]);

    let value = serialized(&out);
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("outcome object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, DEFAULT_TOP_LEVEL_KEYS, "{value}");
    for hit in value["hits"].as_array().expect("hits") {
        assert!(
            hit.get("entity_summary").is_none(),
            "the summary never appears on a hit: {hit}"
        );
    }
}

#[test]
fn an_explicitly_default_request_matches_the_default_call() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "alpha memory");
    fx::put_lexical(&store, &entry, "alpha memory token");
    let retriever = fx::retriever(&store);

    let mut explicit = request("bank", "alpha");
    explicit.min_scores = MinScores::default();
    explicit.include_entities = true;
    explicit.entity_summary = false;

    let default = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("default");
    let spelled_out = retriever.retrieve(&explicit).expect("explicit defaults");
    // Timings differ between calls; everything else must be identical.
    let mut a = serialized(&default);
    let mut b = serialized(&spelled_out);
    a.as_object_mut().expect("outcome object").remove("metrics");
    b.as_object_mut().expect("outcome object").remove("metrics");
    assert_eq!(a, b, "an explicitly-default request is the default call");
}

#[test]
fn the_semantic_floor_prunes_only_the_dense_arm() {
    let store = fx::store();
    let near = fx::vector(1);
    let far = fx::vector(9);
    // Near: dense-only, at similarity 1.0. Far: dense-only, well below it.
    put_both(
        &store,
        &fx::item("NEAR", "bank", "near memory"),
        "unmatched lexical text near",
        near.clone(),
    );
    put_both(
        &store,
        &fx::item("FAR", "bank", "far memory"),
        "unmatched lexical text far",
        far.clone(),
    );
    // The dense floor must not touch the lexical arm: this hit is lexical-only.
    fx::put_lexical(
        &store,
        &fx::item("LEX", "bank", "lexical memory"),
        "alpha lexical",
    );

    let near_similarity =
        crate::stage_score::semantic_from_distance(dense_distance(&store, "bank", &near, "NEAR"));
    let far_similarity =
        crate::stage_score::semantic_from_distance(dense_distance(&store, "bank", &near, "FAR"));
    assert!(
        near_similarity > far_similarity,
        "fixture distances must differ: {near_similarity} vs {far_similarity}"
    );
    let floor = f64::midpoint(near_similarity, far_similarity);

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: near.clone(),
    }));
    let mut req = request("bank", "alpha");
    req.min_scores.semantic = Some(floor);
    let out = retriever.retrieve(&req).expect("floored retrieve");

    let mut kept = hit_ids(&out);
    kept.sort();
    assert_eq!(
        kept,
        vec!["LEX".to_owned(), "NEAR".to_owned()],
        "only the dense candidate below the floor is gone; the lexical hit is untouched"
    );
    assert_eq!(
        out.dense_candidates, 1,
        "the dense counter counts the arm's survivors"
    );
    let lexical = out
        .hits
        .iter()
        .find(|hit| hit.hit.item_id == "LEX")
        .expect("LEX hit");
    assert_eq!(
        lexical.scores.semantic, None,
        "a lexical-only hit reports null, never a fabricated 0"
    );
    assert!(
        lexical.scores.keyword.is_some(),
        "the lexical arm scored it: {lexical:?}"
    );
}

#[test]
fn a_candidate_both_arms_returned_survives_on_the_surviving_arm() {
    let store = fx::store();
    let near = fx::vector(1);
    let far = fx::vector(9);
    // This hit is returned by both arms but sits below the semantic floor in the
    // dense arm, so it survives on its lexical arm alone.
    put_both(
        &store,
        &fx::item("BOTH", "bank", "both arms"),
        "alpha shared token",
        far.clone(),
    );
    put_both(
        &store,
        &fx::item("NEAR", "bank", "near memory"),
        "unmatched near",
        near.clone(),
    );

    let far_similarity =
        crate::stage_score::semantic_from_distance(dense_distance(&store, "bank", &near, "BOTH"));
    let near_similarity =
        crate::stage_score::semantic_from_distance(dense_distance(&store, "bank", &near, "NEAR"));
    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: near.clone(),
    }));
    let mut req = request("bank", "alpha");
    req.min_scores.semantic = Some(f64::midpoint(near_similarity, far_similarity));
    let out = retriever.retrieve(&req).expect("floored retrieve");

    assert!(
        hit_ids(&out).contains(&"BOTH".to_owned()),
        "{:?}",
        hit_ids(&out)
    );
    let both = out
        .hits
        .iter()
        .find(|hit| hit.hit.item_id == "BOTH")
        .expect("BOTH hit");
    assert_eq!(
        both.scores.semantic, None,
        "the dense arm dropped it, so no dense value is reported: {both:?}"
    );
    assert!(both.dense_rank.is_none(), "no dense rank: {both:?}");
    assert!(both.scores.keyword.is_some(), "{both:?}");
}

#[test]
fn the_keyword_floor_prunes_only_the_lexical_arm() {
    let store = fx::store();
    // A short document that matches both query terms outranks a long document
    // with the same terms under FTS5's length normalization, so the two raw
    // lexical scores genuinely differ.
    let short = "alpha beta";
    let long = "alpha beta filler filler filler filler filler filler filler filler filler filler";
    fx::put_lexical(&store, &fx::item("SHORT", "bank", "short doc"), short);
    fx::put_lexical(&store, &fx::item("LONG", "bank", "long doc"), long);
    // The keyword floor must not touch the dense arm: this hit is dense-only.
    fx::put_vector(
        &store,
        &fx::item("DENSE", "bank", "dense only"),
        fx::vector(4),
    );

    let short_score = lexical_score(&store, "bank", "alpha beta", "SHORT");
    let long_score = lexical_score(&store, "bank", "alpha beta", "LONG");
    assert!(
        short_score > long_score,
        "fixture lexical scores must differ: {short_score} vs {long_score}"
    );

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: fx::vector(4),
    }));
    let mut req = request("bank", "alpha beta");
    req.min_scores.keyword = Some(f64::midpoint(short_score, long_score));
    let out = retriever.retrieve(&req).expect("floored retrieve");

    let ids = hit_ids(&out);
    assert!(ids.contains(&"SHORT".to_owned()), "{ids:?}");
    assert!(!ids.contains(&"LONG".to_owned()), "{ids:?}");
    assert!(ids.contains(&"DENSE".to_owned()), "{ids:?}");
    assert_eq!(out.lexical_candidates, 1, "the lexical arm kept one");
    let dense = out
        .hits
        .iter()
        .find(|hit| hit.hit.item_id == "DENSE")
        .expect("DENSE hit");
    assert_eq!(dense.scores.keyword, None, "the dense arm is untouched");
    assert!(dense.scores.semantic.is_some(), "{dense:?}");
}
