// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for rerank client tests: response parsing
//! (`index` and `scores` shapes), HTTP sidecar failure behavior (
//! support), the Cohere-compatible adapter wire shape, and the reranker
//! factory selection rules.
//!
//! ## Owns
//! - `parse_rerank_response` permutation/fail-closed assertions, adapter
//!   wire shapes, factory selection, and bearer masking
//!
//! ## Does not own
//! - Score-retention and normalization assertions (`rerank_score_tests`)
//! - End-to-end rerank ordering in the retriever (`hybrid_rerank_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::rerank::{CohereReranker, HttpReranker, Reranker, build_reranker_from};
use crate::rerank_parse::{RerankedDoc, parse_rerank_response};
use clio_types::ErrorCode;
use serde_json::{Value, json};
use std::time::Duration;

fn docs() -> Vec<String> {
    vec!["alpha".into(), "beta".into(), "gamma".into()]
}

pub(crate) fn indices(docs: &[RerankedDoc]) -> Vec<usize> {
    docs.iter().map(|d| d.index).collect()
}

pub(crate) fn scores(docs: &[RerankedDoc]) -> Vec<Option<f64>> {
    docs.iter().map(|d| d.score).collect()
}

fn http_ok(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn cohere_body(pairs: &[(usize, f64)]) -> Value {
    serde_json::json!({
        "results": pairs
            .iter()
            .map(|(i, s)| serde_json::json!({"index": i, "relevance_score": s}))
            .collect::<Vec<_>>()
    })
}

#[test]
fn parse_scores_argsorts_descending() {
    let value = json!({"scores": [0.1, 0.9, 0.5]});
    let order = parse_rerank_response(&value, 3).expect("parse");
    assert_eq!(indices(&order), vec![1, 2, 0]);
}

#[test]
fn parse_scores_ties_keep_index_order() {
    let value = json!({"scores": [0.5, 0.5, 0.1]});
    let order = parse_rerank_response(&value, 3).expect("parse");
    assert_eq!(indices(&order), vec![0, 1, 2]);
}

#[test]
fn parse_rejects_malformed_bodies() {
    assert!(parse_rerank_response(&json!({}), 2).is_err());
    assert!(
        parse_rerank_response(
            &json!({"results": [{"index": 5, "relevance_score": 0.1}]}),
            2
        )
        .is_err()
    );
    assert!(parse_rerank_response(&json!({"results": [{"index": 0, "relevance_score": 0.1}, {"index": 0, "relevance_score": 0.2}]}), 2).is_err());
    assert!(parse_rerank_response(&json!({"results": [{}]}), 2).is_err());
    assert!(parse_rerank_response(&json!({"scores": ["x"]}), 2).is_err());
}

/// Compatibility semantics: an entry that carries an index but no numeric
/// score still reorders exactly as it did before scores were read. Its score
/// is `None`, never a fabricated value.
#[test]
fn parse_keeps_entries_without_a_numeric_score_in_order() {
    let index_only = parse_rerank_response(&json!([{"index": 1}]), 3).expect("index-only entry");
    assert_eq!(indices(&index_only), vec![1, 0, 2]);
    assert_eq!(scores(&index_only), vec![None, None, None]);

    let non_numeric =
        parse_rerank_response(&json!([{"index": 2, "score": "x"}]), 3).expect("non-numeric score");
    assert_eq!(indices(&non_numeric), vec![2, 0, 1]);
    assert_eq!(scores(&non_numeric), vec![None, None, None]);

    let results_index_only =
        parse_rerank_response(&json!({"results": [{"index": 2}, {"index": 0}]}), 3)
            .expect("results[] index-only entries");
    assert_eq!(indices(&results_index_only), vec![2, 0, 1]);
    assert_eq!(scores(&results_index_only), vec![None, None, None]);
}

#[test]
fn parse_bare_array_rejects_duplicate_and_out_of_range() {
    let dup = json!([{"index": 1, "score": 0.9}, {"index": 1, "score": 0.2}]);
    let err = parse_rerank_response(&dup, 3).expect_err("duplicate");
    assert!(err.message.contains("duplicate"));
    let oob = json!([{"index": 3, "score": 0.9}]);
    let err = parse_rerank_response(&oob, 3).expect_err("out of range");
    assert!(err.message.contains("out of range"));
}

#[test]
fn parse_bare_array_rejects_missing_or_non_integer_index() {
    let missing = json!([{"score": 0.9}]);
    let err = parse_rerank_response(&missing, 3).expect_err("missing index");
    assert!(err.message.contains("integer"));
    assert!(parse_rerank_response(&json!([{"index": -1}]), 3).is_err());
    assert!(parse_rerank_response(&json!([{"index": 1.5}]), 3).is_err());
    assert!(parse_rerank_response(&json!([{"index": "1"}]), 3).is_err());
}

#[test]
fn client_construction_fails_closed() {
    assert!(HttpReranker::new("", None).is_err());
    assert!(HttpReranker::new("ftp://sidecar", None).is_err());
    let mut client = HttpReranker::new("http://127.0.0.1:1", None).expect("build");
    client.set_timeout(Duration::from_millis(50));
    assert!(format!("{client:?}").contains("HttpReranker"));
}

#[test]
fn empty_shortlist_short_circuits() {
    let client = HttpReranker::new(fx::dead_url(), None).expect("build");
    let order = client.rerank("query", &[], 5).expect("empty");
    let positions = indices(&order);
    assert!(positions.is_empty(), "{positions:?}");
}

#[test]
fn dead_sidecar_fails_open_to_caller() {
    let client = HttpReranker::new(fx::dead_url(), None).expect("build");
    assert!(client.rerank("query", &docs(), 3).is_err());
}

#[test]
fn tei_adapter_posts_query_texts_and_pinned_raw_scores() {
    let (url, rx, handle) = fx::fake_http_capture_server(http_ok(&json!([
        {"index": 1, "score": 0.9},
        {"index": 0, "score": 0.4}
    ])));
    let client = HttpReranker::new(url, None).expect("build");
    let order = client.rerank("needle", &docs(), 3).expect("rerank");
    assert_eq!(indices(&order), vec![1, 0, 2]);
    assert_eq!(scores(&order), vec![Some(0.9), Some(0.4), None]);
    let request = rx.recv().expect("captured request");
    assert!(request.starts_with("POST /rerank "));
    assert!(request.contains("\"query\":\"needle\""));
    assert!(request.contains("\"texts\""));
    // `raw_scores: false` is pinned so the sidecar always returns its
    // sigmoid score, even if the server default ever drifts.
    assert!(request.contains("\"raw_scores\":false"));
    assert!(!request.contains("\"documents\""));
    assert!(!request.contains("\"top_k\""));
    handle.join().expect("server join");
}

#[test]
fn tei_http_failure_message_carries_no_transport_label() {
    // The transport reports non-2xx neutrally; the rerank adapter must not
    // reintroduce another subsystem's label into its error text.
    let body = "{}";
    let response = format!(
        "HTTP/1.1 422 Unprocessable Entity\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (url, _rx, handle) = fx::fake_http_capture_server(response);
    let client = HttpReranker::new(url, None).expect("build");
    let err = client.rerank("query", &docs(), 3).expect_err("http 422");
    assert!(!err.message.contains("embed"));
    handle.join().expect("server join");
}

#[test]
fn live_sidecar_reorders() {
    let response = http_ok(&json!([
        {"index": 1, "score": 0.9},
        {"index": 2, "score": 0.5},
        {"index": 0, "score": 0.1}
    ]));
    let (url, handle) = fx::fake_http_server(response);
    let client = HttpReranker::new(url, Some("secret".into())).expect("build");
    let order = client.rerank("query", &docs(), 3).expect("rerank");
    assert_eq!(indices(&order), vec![1, 2, 0]);
    handle.join().expect("server join");
}

#[test]
fn cohere_adapter_posts_v1_rerank_with_top_n_model_and_bearer() {
    let (url, rx, handle) =
        fx::fake_http_capture_server(http_ok(&cohere_body(&[(1, 0.9), (2, 0.4), (0, 0.1)])));
    let client =
        CohereReranker::new(url, "rerank-test-model", Some("sk-secret-key".into())).expect("build");
    let order = client.rerank("query", &docs(), 3).expect("rerank");
    assert_eq!(indices(&order), vec![1, 2, 0]);
    assert_eq!(scores(&order), vec![Some(0.9), Some(0.4), Some(0.1)]);
    let request = rx.recv().expect("captured request");
    assert!(request.starts_with("POST /v1/rerank "));
    assert!(request.contains("\"query\":\"query\""));
    assert!(request.contains("\"top_n\":3"));
    assert!(request.contains("\"model\":\"rerank-test-model\""));
    assert!(request.contains("authorization: Bearer sk-secret-key"));
    handle.join().expect("server join");
}

#[test]
fn cohere_permutation_equals_tei_results_parse() {
    // Same best-first order as the TEI results[] parser: both reuse
    // parse_rerank_response; omitted documents append in original order.
    let expected =
        parse_rerank_response(&json!({"results": [{"index": 2, "relevance_score": 0.8}, {"index": 0, "relevance_score": 0.2}]}), 3)
            .expect("tei parse");
    let (url, _rx, handle) =
        fx::fake_http_capture_server(http_ok(&cohere_body(&[(2, 0.8), (0, 0.2)])));
    let client = CohereReranker::new(url, "m", None).expect("build");
    let order = client.rerank("query", &docs(), 3).expect("rerank");
    assert_eq!(order, expected);
    assert_eq!(indices(&order), vec![2, 0, 1]);
    handle.join().expect("server join");
}

#[test]
fn cohere_url_with_trailing_v1_hits_v1_rerank_once() {
    // Operators following OpenAI-style base-URL conventions may set
    // CLIO_RERANK_URL with a /v1 suffix; the adapter must not double the segment.
    let (url, rx, handle) = fx::fake_http_capture_server(http_ok(&cohere_body(&[(0, 0.9)])));
    let client = CohereReranker::new(format!("{url}/v1"), "m", None).expect("build");
    client.rerank("query", &docs(), 3).expect("rerank");
    assert!(rx.recv().expect("captured").starts_with("POST /v1/rerank "));
    handle.join().expect("server join");
}

#[test]
fn cohere_adapter_top_n_is_clamped_to_doc_count() {
    let (url, rx, handle) =
        fx::fake_http_capture_server(http_ok(&cohere_body(&[(0, 0.9), (1, 0.4)])));
    let client = CohereReranker::new(url, "m", None).expect("build");
    let order = client.rerank("query", &docs()[..2], 9).expect("rerank");
    assert_eq!(indices(&order), vec![0, 1]);
    assert!(rx.recv().expect("captured").contains("\"top_n\":2"));
    handle.join().expect("server join");
}

#[test]
fn cohere_adapter_rejects_bad_indices_fail_closed() {
    let dup = http_ok(&cohere_body(&[(0, 0.9), (0, 0.4)]));
    let (url, _rx, handle) = fx::fake_http_capture_server(dup);
    let client = CohereReranker::new(url, "m", None).expect("build");
    let err = client.rerank("query", &docs(), 3).expect_err("duplicate");
    assert!(err.message.contains("duplicate"));
    handle.join().expect("server join");
    let oob = http_ok(&cohere_body(&[(9, 0.9)]));
    let (url, _rx, handle) = fx::fake_http_capture_server(oob);
    let client = CohereReranker::new(url, "m", None).expect("build");
    let err = client
        .rerank("query", &docs(), 3)
        .expect_err("out of range");
    assert!(err.message.contains("out of range"));
    handle.join().expect("server join");
}

#[test]
fn cohere_empty_documents_skip_the_request() {
    let client = CohereReranker::new(fx::dead_url(), "m", None).expect("build");
    let order = client.rerank("query", &[], 5).expect("empty");
    let positions = indices(&order);
    assert!(positions.is_empty(), "{positions:?}");
}

#[test]
fn cohere_construction_fails_closed() {
    // Each generic instantiation of `new` must fail closed on its own paths.
    assert!(CohereReranker::new("", "m", None).is_err());
    assert!(CohereReranker::new("http://127.0.0.1:1", "", None).is_err());
    assert!(CohereReranker::new("ftp://127.0.0.1:1", "m", None).is_err());
    assert!(CohereReranker::new(String::new(), "m", None).is_err());
    assert!(CohereReranker::new(String::new(), String::new(), None).is_err());
    assert!(CohereReranker::new("http://127.0.0.1:1".to_string(), String::new(), None).is_err());
}

#[test]
fn cohere_set_timeout_overrides_deadline() {
    let mut client = CohereReranker::new("http://127.0.0.1:1", "m", None).expect("build");
    client.set_timeout(Duration::from_millis(50));
    assert!(format!("{client:?}").contains("50ms"));
}

#[test]
fn http_construction_fails_closed_on_string_url() {
    assert!(HttpReranker::new(String::new(), None).is_err());
}

#[test]
fn http_debug_masks_the_bearer() {
    let client =
        HttpReranker::new("http://127.0.0.1:1", Some("sidecar-secret".into())).expect("build");
    let dbg = format!("{client:?}");
    assert!(dbg.contains("HttpReranker"));
    assert!(dbg.contains("****"));
    assert!(!dbg.contains("sidecar-secret"));
}

#[test]
fn cohere_debug_masks_the_bearer() {
    let client = CohereReranker::new("http://127.0.0.1:1", "m", Some("sk-secret-key".into()))
        .expect("build");
    let dbg = format!("{client:?}");
    assert!(dbg.contains("CohereReranker"));
    assert!(dbg.contains("****"));
    assert!(!dbg.contains("sk-secret-key"));
}

#[test]
fn cohere_transport_errors_carry_no_key_material() {
    let client =
        CohereReranker::new(fx::dead_url(), "m", Some("sk-secret-key".into())).expect("build");
    let err = client.rerank("query", &docs(), 3).expect_err("dead server");
    assert!(!err.message.contains("sk-secret-key"));
}

#[test]
fn factory_without_url_is_none() {
    let eff = json!({"rerank": {"url": "", "model": "", "provider": ""}});
    assert!(build_reranker_from(&eff, None).expect("build").is_none());
}

#[test]
fn factory_selects_teiranker_by_provider() {
    let (url, rx, handle) = fx::fake_http_capture_server(http_ok(&json!([
        {"index": 1, "score": 0.9},
        {"index": 0, "score": 0.4}
    ])));
    let tei = json!({
        "rerank": {"url": url, "model": "m", "provider": "tei", "max_batch": 2}
    });
    let r = build_reranker_from(&tei, None).expect("tei").expect("some");
    assert_eq!(r.max_batch_size(), 2);
    let order = r.rerank("query", &docs(), 3).expect("rerank");
    assert_eq!(indices(&order), vec![1, 0, 2]); // omitted doc appends in original order
    // TEI wire shape: POST {url}/rerank with {query, texts}; no top_k/documents.
    let request = rx.recv().expect("captured request");
    assert!(request.starts_with("POST /rerank "));
    assert!(request.contains("\"texts\""));
    assert!(!request.contains("\"top_k\""));
    assert!(!request.contains("\"documents\""));
    handle.join().expect("server join");
}

#[test]
fn factory_omitted_provider_uses_the_tei_default() {
    // An omitted provider key and the empty system default are both the unset
    // representation; both resolve to the explicit tei default and the TEI route.
    // Each case gets its own capture server: it serves one connection.
    for provider in [None, Some("")] {
        let (url, rx, handle) = fx::fake_http_capture_server(http_ok(&cohere_body(&[(0, 0.9)])));
        let mut service = json!({"url": url, "model": "m"});
        if let Some(p) = provider {
            service["provider"] = json!(p);
        }
        let eff = json!({"rerank": service});
        let r = build_reranker_from(&eff, None)
            .expect("unset")
            .expect("some");
        r.rerank("query", &docs(), 3).expect("rerank");
        // TEI path, not the Cohere /v1/rerank path.
        assert!(rx.recv().expect("captured").starts_with("POST /rerank "));
        handle.join().expect("server join");
    }
}

#[test]
fn factory_selects_cohere_with_bearer() {
    let (url, rx, handle) = fx::fake_http_capture_server(http_ok(&cohere_body(&[(0, 0.9)])));
    // The configurable cap belongs to the TEI sidecar. Cohere keeps one
    // request for the shortlist because its API has a different request limit.
    let cohere = json!({"rerank": {"url": url, "model": "rerank-v3.5", "provider": "cohere"}});
    let r = build_reranker_from(&cohere, Some("sk-factory-key".into()))
        .expect("cohere")
        .expect("some");
    assert_eq!(r.max_batch_size(), usize::MAX);
    r.rerank("query", &docs(), 3).expect("rerank");
    let request = rx.recv().expect("captured request");
    assert!(request.starts_with("POST /v1/rerank "));
    assert!(request.contains("authorization: Bearer sk-factory-key"));
    handle.join().expect("server join");
}

#[test]
fn factory_fails_closed_on_bad_provider_config() {
    let no_model = json!({"rerank": {"url": "http://127.0.0.1:1", "model": "", "provider": "tei"}});
    let e = build_reranker_from(&no_model, None)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfigCorrupt);
    assert!(e.message.contains("rerank.model is required"));

    let cohere_no_key = json!({"rerank": {"url": "https://api.cohere.com", "model": "rerank-v3.5", "provider": "cohere"}});
    let e = build_reranker_from(&cohere_no_key, None)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfigCorrupt);
    assert!(e.message.contains("credentials.rerank_api_key"));

    let unknown =
        json!({"rerank": {"url": "http://127.0.0.1:1", "model": "m", "provider": "openai"}});
    let e = build_reranker_from(&unknown, None).map(|_| ()).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfigCorrupt);
    assert!(e.message.contains("tei|cohere"));

    let bad_url = json!({"rerank": {"url": "ftp://x", "model": "m", "provider": "tei"}});
    assert!(build_reranker_from(&bad_url, None).is_err());
}

#[test]
fn runtime_factory_without_rerank_config_is_none() {
    // "No rerank config means no reranker" must hold on any host, so build the
    // runtime with an injected empty environment instead of `Runtime::new()`.
    // `Runtime::new()` also layers the deployment file, and on a machine that
    // has a rerank sidecar configured the old assertion failed under a bare
    // `cargo test` while passing under `make`, which sets a hermetic overlay.
    // The old guard only checked two env vars, so it did not skip there.
    let runtime = clio_config::Runtime::with_env(&|_| None);
    assert!(
        build_reranker_from(&runtime.effective_raw(), runtime.bearer_for("rerank"))
            .expect("build")
            .is_none()
    );
}
