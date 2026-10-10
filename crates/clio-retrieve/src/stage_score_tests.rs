// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-hit `scores` object tests: the dense
//! distance→similarity transform boundaries, the serialized object shape, the
//! null-when-absent rule, and per-stage values surviving fuse → dedup → finalize.
//!
//! ## Owns
//! - Unit assertions over `semantic_from_distance` and `HitScores`
//! - Contract, determinism, empty-recall, and PII-safety assertions for `scores`
//! - End-to-end assertions that per-stage values survive fuse → dedup → finalize
//!
//! ## Does not own
//! - Fusion math (`fusion_tests`) or binding-level payloads (`clio-mcp`,
//!   `clio` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::fusion::{DEFAULT_RRF_K, FusedHit, RankList, rrf_fuse};
use crate::stage_score::{HitScores, semantic_from_distance};
use crate::types::{RetrieveOutcome, RetrieveRequest};
use clio_index::{DenseSearchArgs, LexicalSearchArgs, dense_search, lexical_search};
use clio_store::{IndexStore, SqliteStore};
use clio_types::MemoryItem;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Request with a frozen clock so recency boosts stay deterministic.
fn request(bank: &str, query: &str) -> RetrieveRequest {
    let mut req = RetrieveRequest::new(bank, query);
    req.now = Some("2026-01-02T00:00:00Z".into());
    req
}

/// Persist one item with both a dense vector and a lexical document.
fn put_both(store: &Arc<SqliteStore>, entry: &MemoryItem, text: &str, embedding: Vec<f32>) {
    fx::put_vector(store, entry, embedding);
    store
        .put_lexical_doc(&entry.bank_id, &entry.id, text)
        .expect("put lexical doc");
}

/// Raw cosine distance the dense leg sees for one item.
fn dense_distance(store: &Arc<SqliteStore>, bank: &str, query: &[f32], id: &str) -> f64 {
    let hits = dense_search(
        store.as_ref(),
        &DenseSearchArgs {
            bank,
            query,
            domains: Vec::new(),
            limit: 10,
        },
    )
    .expect("dense search");
    hits.iter()
        .find(|hit| hit.item_id == id)
        .expect("dense hit")
        .distance
}

/// Raw backend lexical score the lexical leg sees for one item.
fn lexical_score(store: &Arc<SqliteStore>, bank: &str, query: &str, id: &str) -> f64 {
    let hits = lexical_search(
        store.as_ref(),
        &LexicalSearchArgs {
            bank,
            query,
            domains: Vec::new(),
            limit: 10,
        },
    )
    .expect("lexical search");
    hits.iter()
        .find(|hit| hit.item_id == id)
        .expect("lexical hit")
        .score
}

/// The observable per-hit fields the determinism assertions compare.
#[derive(Debug, PartialEq)]
struct HitSnapshot {
    id: String,
    score: f64,
    scores: HitScores,
    dense_rank: Option<usize>,
    lexical_rank: Option<usize>,
}

fn snapshots(out: &RetrieveOutcome) -> Vec<HitSnapshot> {
    out.hits
        .iter()
        .map(|hit| HitSnapshot {
            id: hit.hit.item_id.clone(),
            score: hit.score,
            scores: hit.scores,
            dense_rank: hit.dense_rank,
            lexical_rank: hit.lexical_rank,
        })
        .collect()
}

#[test]
fn semantic_transform_maps_distance_to_clamped_similarity() {
    assert_eq!(semantic_from_distance(0.0), 1.0, "exact match");
    assert!((semantic_from_distance(0.25) - 0.75).abs() < 1e-12);
    assert_eq!(semantic_from_distance(1.0), 0.0, "orthogonal pair");
    // Distances above 1 would go negative; the clamp floors them at 0.
    assert_eq!(semantic_from_distance(1.5), 0.0);
    assert_eq!(semantic_from_distance(2.0), 0.0, "opposite pair");
}

#[test]
fn scores_object_serializes_the_documented_shape() {
    let scores = HitScores {
        final_score: 0.5,
        reranker: None,
        semantic: Some(0.25),
        keyword: Some(3.5),
        age_factor: 1.0,
        trust_factor: 0.875,
    };
    let value = serde_json::to_value(scores).expect("serialize scores");
    assert_eq!(
        value,
        json!({"final": 0.5, "reranker": null, "semantic": 0.25, "keyword": 3.5, "age_factor": 1.0, "trust_factor": 0.875})
    );
}

#[test]
fn absent_arm_values_serialize_as_null_not_zero() {
    let scores = HitScores {
        final_score: 0.0,
        reranker: None,
        semantic: None,
        keyword: None,
        age_factor: 1.0,
        trust_factor: 1.0,
    };
    let value = serde_json::to_value(scores).expect("serialize scores");
    assert_eq!(value["reranker"], json!(null));
    assert_eq!(value["semantic"], json!(null));
    assert_eq!(value["keyword"], json!(null));
}

#[test]
fn per_stage_scores_survive_fuse_dedup_and_finalize() {
    let store = fx::store();
    let query_vector = fx::vector(1);
    let both = fx::item("A", "bank", "alpha memory A");
    put_both(&store, &both, "alpha memory token A", query_vector.clone());
    let lexical_only = fx::item("B", "bank", "alpha memory B");
    fx::put_lexical(&store, &lexical_only, "alpha memory token B");

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector.clone(),
    }));
    let out = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");

    let hit = out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "A")
        .expect("A hit");
    let expected_semantic =
        semantic_from_distance(dense_distance(&store, "bank", &query_vector, "A"));
    assert_eq!(hit.scores.semantic, Some(expected_semantic));
    assert_eq!(
        hit.scores.keyword,
        Some(lexical_score(&store, "bank", "alpha", "A"))
    );
    assert_eq!(
        hit.scores.final_score, hit.score,
        "final mirrors the top-level score"
    );
    assert_eq!(hit.scores.reranker, None);
}

#[test]
fn dense_distance_above_one_clamps_similarity_to_zero() {
    let store = fx::store();
    let stored = fx::vector(1);
    // A stored vector and its negation have cosine similarity -1, i.e. a
    // distance of 2, which would yield a negative similarity without the clamp.
    let query_vector: Vec<f32> = stored.iter().map(|value| -*value).collect();
    let entry = fx::item("N", "bank", "opposite direction");
    fx::put_vector(&store, &entry, stored);

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector.clone(),
    }));
    let out = retriever
        .retrieve(&request("bank", "opposite"))
        .expect("retrieve");
    let hit = out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "N")
        .expect("N hit");
    let distance = dense_distance(&store, "bank", &query_vector, "N");
    assert!(distance > 1.0, "fixture distance must exceed 1: {distance}");
    assert_eq!(
        hit.scores.semantic,
        Some(0.0),
        "negative similarity clamps to 0"
    );
    assert!(hit.scores.semantic.is_some_and(|value| value >= 0.0));
}

#[test]
fn absent_arm_scores_are_null_not_zero() {
    let store = fx::store();
    let query_vector = fx::vector(3);
    let dense_only = fx::item("D", "bank", "dense only");
    fx::put_vector(&store, &dense_only, query_vector.clone());
    let lexical_only = fx::item("L", "bank", "lexical only");
    fx::put_lexical(&store, &lexical_only, "alpha lexical");

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector,
    }));
    let out = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");

    let dense = out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "D")
        .expect("D hit");
    assert!(dense.scores.semantic.is_some());
    assert_eq!(dense.scores.keyword, None);
    let lexical = out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "L")
        .expect("L hit");
    assert!(lexical.scores.keyword.is_some());
    assert_eq!(lexical.scores.semantic, None);

    let dense_json = serde_json::to_value(dense).expect("serialize D");
    assert!(
        dense_json["scores"]["keyword"].is_null(),
        "absent arm serializes as null, not 0: {}",
        dense_json["scores"]["keyword"]
    );
    let lexical_json = serde_json::to_value(lexical).expect("serialize L");
    assert!(
        lexical_json["scores"]["semantic"].is_null(),
        "absent arm serializes as null, not 0: {}",
        lexical_json["scores"]["semantic"]
    );
}

#[test]
fn reranker_keeps_scores_and_existing_order_behavior() {
    let store = fx::store();
    let query_vector = fx::vector(4);
    for id in ["A", "B", "C"] {
        let entry = fx::item(id, "bank", &format!("shared token {id}"));
        put_both(
            &store,
            &entry,
            &format!("shared token {id}"),
            query_vector.clone(),
        );
    }
    let plain = fx::retriever(&store);
    let fused = plain
        .retrieve(&request("bank", "shared"))
        .expect("retrieve");
    let mut reranked = fx::retriever(&store);
    reranked.set_reranker(Arc::new(fx::ReverseReranker));
    let out = reranked
        .retrieve(&request("bank", "shared"))
        .expect("retrieve");

    assert!(out.reranked);
    let mut expected: Vec<String> = fused.hits.iter().map(|h| h.hit.item_id.clone()).collect();
    expected.reverse();
    let actual: Vec<String> = out.hits.iter().map(|h| h.hit.item_id.clone()).collect();
    assert_eq!(actual, expected, "reverse-shortlist behavior is unchanged");

    // The reranker's normalized relevance rides the reordered hits; the
    // score is bound to the original fused rank (doc index), not the new
    // position. `ReverseReranker` scores doc index r as `0.9 - 0.1 * r`.
    for (rank, hit) in fused.hits.iter().enumerate() {
        let expected_score = 0.9 - rank as f64 * 0.1;
        let after = out
            .hits
            .iter()
            .find(|h| h.hit.item_id == hit.hit.item_id)
            .expect("reranked hit present");
        assert_eq!(after.scores.reranker, Some(expected_score), "{hit:?}");
    }
    for (before, after) in fused.hits.iter().zip(out.hits.iter().rev()) {
        assert_eq!(before.hit.item_id, after.hit.item_id);
        assert_eq!(before.score, after.score, "rerank reorders, never rescores");
        // `final`, `semantic`, and `keyword` are unchanged by rerank; only
        // the `reranker` slot is newly populated.
        assert_eq!(before.scores.final_score, after.scores.final_score);
        assert_eq!(before.scores.semantic, after.scores.semantic);
        assert_eq!(before.scores.keyword, after.scores.keyword);
    }
}

#[test]
fn identical_inputs_produce_identical_scores_and_ranks() {
    let store = fx::store();
    let query_vector = fx::vector(5);
    let both = fx::item("A", "bank", "alpha memory A");
    put_both(&store, &both, "alpha memory token A", query_vector.clone());
    let lexical_only = fx::item("B", "bank", "alpha memory B");
    fx::put_lexical(&store, &lexical_only, "alpha memory token B");

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector,
    }));
    let first = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("first run");
    let second = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("second run");
    assert_eq!(snapshots(&first), snapshots(&second));

    let dense_ids = vec!["x".to_owned(), "y".to_owned()];
    let dense_scores = [0.1_f64, 0.9_f64];
    let lexical_ids = vec!["y".to_owned(), "x".to_owned()];
    let lexical_scores = [2.5_f64, 1.5_f64];
    let lists = [
        RankList {
            weight: 1.0,
            ids: &dense_ids,
            scores: Some(&dense_scores),
        },
        RankList {
            weight: 1.0,
            ids: &lexical_ids,
            scores: Some(&lexical_scores),
        },
    ];
    let one = rrf_fuse(&lists, DEFAULT_RRF_K);
    let two = rrf_fuse(&lists, DEFAULT_RRF_K);
    assert_eq!(one, two, "fused scores and ordering repeat");
    assert_eq!(
        one.iter().map(FusedHit::ranks).collect::<Vec<_>>(),
        two.iter().map(FusedHit::ranks).collect::<Vec<_>>()
    );
}

#[test]
fn empty_recall_stays_empty_without_panicking() {
    let store = fx::store();
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("bank", "nothing"))
        .expect("retrieve");
    assert!(out.hits.is_empty(), "{:?}", out.hits);
    assert_eq!(out.fused_candidates, 0);
    let value = serde_json::to_value(&out).expect("serialize");
    assert_eq!(value["hits"], json!([]));
}

#[test]
fn serialized_scores_expose_only_numeric_stage_fields() {
    let store = fx::store();
    let query_vector = fx::vector(6);
    let entry = fx::item("A", "bank", "secret gist content");
    put_both(&store, &entry, "secret gist content", query_vector.clone());
    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector,
    }));
    let out = retriever
        .retrieve(&request("bank", "secret"))
        .expect("retrieve");
    let value = serde_json::to_value(&out.hits[0]).expect("serialize hit");

    let scores = value["scores"].as_object().expect("scores object");
    let keys: BTreeSet<&str> = scores.keys().map(String::as_str).collect();
    // The sorted key set as one string, so a seventh key or a dropped key
    // fails here without the array literal being wrapped.
    assert_eq!(
        keys.into_iter().collect::<Vec<_>>().join(","),
        "age_factor,final,keyword,reranker,semantic,trust_factor"
    );
    for (key, field) in scores {
        assert!(
            field.is_number() || field.is_null(),
            "scores.{key} is numeric or null, got {field}"
        );
    }
    // `age_factor` is always present and finite, so it is a number and never
    // null: serde_json would emit null for a NaN or infinite factor.
    assert!(
        scores["age_factor"].is_number(),
        "age_factor is a finite number, got {}",
        scores["age_factor"]
    );
    let scores_text = serde_json::to_string(&value["scores"]).expect("serialize scores");
    assert!(
        !scores_text.contains("secret"),
        "scores must not carry content: {scores_text}"
    );
    assert!(
        !scores_text.contains("gist"),
        "scores must not carry content: {scores_text}"
    );
    assert_eq!(value["score"], value["scores"]["final"]);
}

#[test]
fn top_level_score_stays_equal_to_final_for_every_hit() {
    let store = fx::store();
    let query_vector = fx::vector(7);
    let both = fx::item("A", "bank", "alpha memory A");
    put_both(&store, &both, "alpha memory token A", query_vector.clone());
    let lexical_only = fx::item("B", "bank", "alpha memory B");
    fx::put_lexical(&store, &lexical_only, "alpha memory token B");

    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: query_vector,
    }));
    let out = retriever
        .retrieve(&request("bank", "alpha"))
        .expect("retrieve");
    assert!(!out.hits.is_empty(), "{:?}", out.hits);
    for hit in &out.hits {
        assert_eq!(hit.scores.final_score, hit.score);
        let value = serde_json::to_value(hit).expect("serialize hit");
        assert_eq!(value["score"], value["scores"]["final"]);
    }
}
