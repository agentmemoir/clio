// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for rerank score-contract tests: score
//! retention for every response shape, the omitted-document `null` rule,
//! normalization boundaries, and the pinned TEI `raw_scores` request field
//! observed through the real adapters.
//!
//! ## Owns
//! - `parse_rerank_response` score-retention and normalization assertions
//!   (bare array, `results[]`, `scores[]`; adapter-level shape tests)
//!
//! ## Does not own
//! - Permutation/fail-closed/config assertions (`rerank_tests`)
//! - End-to-end rerank ordering in the retriever (`hybrid_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::rerank::rerank_tests::{indices, scores};
use crate::rerank::{HttpReranker, Reranker};
use crate::rerank_parse::{normalize_rerank_score, parse_rerank_response};
use serde_json::{Value, json};

fn docs() -> Vec<String> {
    vec!["alpha".into(), "beta".into(), "gamma".into()]
}

fn http_ok(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn parse_results_retains_relevance_scores() {
    let value = json!({"results": [
        {"index": 2, "relevance_score": 0.8},
        {"index": 0, "relevance_score": 0.2}
    ]});
    let order = parse_rerank_response(&value, 3).expect("parse");
    assert_eq!(indices(&order), vec![2, 0, 1]);
    assert_eq!(scores(&order), vec![Some(0.8), Some(0.2), None]);
}
#[test]
fn parse_results_accepts_score_alias_and_keeps_unscored_entries() {
    let aliased = json!({"results": [{"index": 0, "score": 0.5}]});
    let order = parse_rerank_response(&aliased, 2).expect("parse");
    assert_eq!(scores(&order), vec![Some(0.5), None]);
    // Compatibility semantics: an entry without a numeric score keeps its
    // index and order and carries `None`, never a fabricated value.
    for body in [
        json!({"results": [{"index": 1}]}),
        json!({"results": [{"index": 1, "relevance_score": "x"}]}),
        json!({"results": [{"index": 1, "relevance_score": null}]}),
    ] {
        let order = parse_rerank_response(&body, 2).expect("unscored entry");
        assert_eq!(indices(&order), vec![1, 0]);
        assert_eq!(scores(&order), vec![None, None]);
    }
}
#[test]
fn parse_scores_shape_retains_scores() {
    let value = json!({"scores": [0.1, 0.9, 0.5]});
    let order = parse_rerank_response(&value, 3).expect("parse");
    assert_eq!(indices(&order), vec![1, 2, 0]);
    assert_eq!(scores(&order), vec![Some(0.9), Some(0.5), Some(0.1)]);
}
#[test]
fn parse_scores_shorter_than_docs_appends_remainder_without_scores() {
    let value = json!({"scores": [0.1, 0.9]});
    let order = parse_rerank_response(&value, 3).expect("parse");
    assert_eq!(indices(&order), vec![1, 0, 2]);
    assert_eq!(scores(&order), vec![Some(0.9), Some(0.1), None]);
}
#[test]
fn parse_bare_array_trusts_order_and_appends_omitted() {
    let value = json!([{"index": 1, "score": 0.9}, {"index": 0, "score": 0.1}]);
    let order = parse_rerank_response(&value, 3).expect("parse");
    assert_eq!(indices(&order), vec![1, 0, 2]);
    assert_eq!(scores(&order), vec![Some(0.9), Some(0.1), None]);
}
#[test]
fn parse_bare_array_omitted_document_appends_in_original_order() {
    let value = json!([{"index": 2, "score": 0.9}]);
    let order = parse_rerank_response(&value, 3).expect("parse");
    assert_eq!(indices(&order), vec![2, 0, 1]);
    // The provider never scored the appended documents: `None`, never a
    // fabricated value.
    assert_eq!(scores(&order), vec![Some(0.9), None, None]);
}
#[test]
fn normalize_rerank_score_boundaries() {
    // TEI pin: `raw_scores: false` yields sigmoid scores in approx. (0, 1);
    // the identity keeps them exact. Cohere's calibrated `relevance_score`
    // endpoints 0 and 1 pass through unchanged.
    assert_eq!(normalize_rerank_score(0.0), 0.0);
    assert_eq!(normalize_rerank_score(1.0), 1.0);
    assert_eq!(normalize_rerank_score(0.42), 0.42);
    // A non-conforming server that returns raw logits cannot leak an
    // out-of-range value; the clamp is deterministic for identical inputs.
    assert_eq!(normalize_rerank_score(-1.5), 0.0);
    assert_eq!(normalize_rerank_score(2.0), 1.0);
    assert_eq!(normalize_rerank_score(-1.5), normalize_rerank_score(-1.5));
}

#[test]
fn http_adapter_results_shape_yields_scores_through_the_adapter() {
    let (url, _rx, handle) = fx::fake_http_capture_server(http_ok(&json!({"results": [
        {"index": 2, "relevance_score": 0.9},
        {"index": 0, "relevance_score": 0.4}
    ]})));
    let client = HttpReranker::new(url, None).expect("build");
    let order = client.rerank("query", &docs(), 3).expect("rerank");
    assert_eq!(indices(&order), vec![2, 0, 1]);
    assert_eq!(scores(&order), vec![Some(0.9), Some(0.4), None]);
    handle.join().expect("server join");
}

#[test]
fn http_adapter_scores_shape_yields_scores_through_the_adapter() {
    let (url, _rx, handle) =
        fx::fake_http_capture_server(http_ok(&json!({"scores": [0.2, 0.8, 0.5]})));
    let client = HttpReranker::new(url, None).expect("build");
    let order = client.rerank("query", &docs(), 3).expect("rerank");
    assert_eq!(indices(&order), vec![1, 2, 0]);
    assert_eq!(scores(&order), vec![Some(0.8), Some(0.5), Some(0.2)]);
    handle.join().expect("server join");
}

#[test]
fn http_adapter_results_shape_carries_no_score_when_the_provider_omits_it() {
    // Pre-score compatibility semantics through the real adapter: an
    // index-only body still reorders exactly as it did before, and the
    // omitted score stays `None` rather than failing the whole rerank.
    let (url, _rx, handle) =
        fx::fake_http_capture_server(http_ok(&json!({"results": [{"index": 2}, {"index": 0}]})));
    let client = HttpReranker::new(url, None).expect("build");
    let order = client.rerank("query", &docs(), 3).expect("rerank");
    assert_eq!(indices(&order), vec![2, 0, 1]);
    assert_eq!(scores(&order), vec![None, None, None]);
    handle.join().expect("server join");
}

#[test]
fn adapter_clamps_out_of_range_scores_into_the_documented_range() {
    // A non-conforming server that returns raw logits must not leak an
    // out-of-range `reranker` value; the clamp keeps the documented range
    // and stays deterministic for identical inputs.
    let (url, _rx, handle) = fx::fake_http_capture_server(http_ok(&json!([
        {"index": 0, "score": -1.5},
        {"index": 1, "score": 2.0}
    ])));
    let client = HttpReranker::new(url, None).expect("build");
    let order = client.rerank("query", &docs(), 3).expect("rerank");
    // Best-first after the clamp: the above-range score leads, the clamped
    // negative trails it, and the omitted document stays last with `None`.
    assert_eq!(indices(&order), vec![1, 0, 2]);
    assert_eq!(scores(&order), vec![Some(1.0), Some(0.0), None]);
    handle.join().expect("server join");
}
