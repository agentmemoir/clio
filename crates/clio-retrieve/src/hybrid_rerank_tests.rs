// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for hybrid-retrieval rerank behavior tests:
//! rerank ordering and score binding, fail-open fallback on rerank errors,
//! and reranker attach/clear.
//!
//! ## Owns
//! - End-to-end retriever rerank assertions (support): populated
//!   `scores.reranker` on success, `null` for a document the provider omitted,
//!   and `null` on failure or a dead sidecar
//!
//! ## Does not own
//! - Parser/adapter score assertions (`rerank_tests`, `rerank_score_tests`)
//! - General hybrid behavior (`hybrid_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::rerank::HttpReranker;
use crate::rerank::rerank_batch_tests::{batch_server, texts};
use serde_json::{Value, json};
use std::sync::Arc;

fn request(bank: &str, query: &str) -> crate::types::RetrieveRequest {
    crate::types::RetrieveRequest::new(bank, query)
}

fn ids(out: &crate::types::RetrieveOutcome) -> Vec<String> {
    out.hits.iter().map(|h| h.hit.item_id.clone()).collect()
}

fn http_ok(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Lexical document text owned by `index`: every candidate matches the query,
/// and no two candidates are near-duplicates of each other.
fn candidate_text(index: usize) -> String {
    let words: Vec<String> = (0..8).map(|k| format!("w{index}x{k}")).collect();
    format!("shared {}", words.join(" "))
}

/// TEI body for one batch of the shortlist: that batch's documents served in
/// reverse, each scored by the place it takes in the reranked page. The first
/// batch therefore scores above the second, the page is monotone in the served
/// score, and the fused order is not the order that comes back.
fn reranked_batch(start: usize, len: usize, total: usize) -> String {
    let entries: Vec<Value> = (0..len)
        .map(|j| {
            json!({
                "index": len - 1 - j,
                "score": (total - 1 - start - j) as f64 / 100.0,
            })
        })
        .collect();
    http_ok(&Value::Array(entries))
}

#[test]
fn t08_reverse_reranker_reverses_fused_order() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        // Distinct texts: a shared one would make these exact duplicates, and
        // read-time collapse would leave one hit for the reranker to reverse.
        let entry = fx::item(id, "bank", &format!("shared token {id}"));
        fx::put_lexical(&store, &entry, &format!("shared token {id}"));
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
    let mut expected = ids(&fused);
    expected.reverse();
    assert_eq!(ids(&out), expected);
    // Every reranked shortlist hit carries its normalized relevance;
    // `ReverseReranker` scores fused rank r as `0.9 - 0.1 * r`.
    let fused_ids = ids(&fused);
    for hit in &out.hits {
        let fused_rank = fused_ids
            .iter()
            .position(|id| *id == hit.hit.item_id)
            .expect("hit came from the shortlist");
        assert_eq!(hit.scores.reranker, Some(0.9 - fused_rank as f64 * 0.1));
    }
}
#[test]
fn t08_omitted_document_appends_in_fused_order_with_null_reranker() {
    let store = fx::store();
    for id in ["A", "B", "C"] {
        let entry = fx::item(id, "bank", &format!("shared token {id}"));
        fx::put_lexical(&store, &entry, &format!("shared token {id}"));
    }
    let fused = ids(&fx::retriever(&store)
        .retrieve(&request("bank", "shared"))
        .expect("retrieve"));
    assert_eq!(fused.len(), 3, "all three documents must be retrievable");

    // The sidecar scores the first two shortlist documents and omits the
    // third: the omitted document still appends in fused order and its
    // `scores.reranker` stays `null` — never a fabricated value.
    let (url, _rx, handle) = fx::fake_http_capture_server(http_ok(&json!([
        {"index": 1, "score": 0.8},
        {"index": 0, "score": 0.3}
    ])));
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(HttpReranker::new(url, None).expect("client")));
    let out = retriever
        .retrieve(&request("bank", "shared"))
        .expect("retrieve");
    assert!(out.reranked);
    assert_eq!(
        ids(&out),
        vec![fused[1].clone(), fused[0].clone(), fused[2].clone()]
    );
    assert_eq!(out.hits[0].scores.reranker, Some(0.8));
    assert_eq!(out.hits[1].scores.reranker, Some(0.3));
    assert_eq!(out.hits[2].scores.reranker, None, "{:?}", out.hits[2]);
    handle.join().expect("server join");
}
#[test]
fn t08_rerank_failure_keeps_fused_order() {
    let store = fx::store();
    for id in ["A", "B"] {
        let entry = fx::item(id, "bank", "shared token");
        fx::put_lexical(&store, &entry, "shared token");
    }
    let plain = fx::retriever(&store);
    let fused = ids(&plain
        .retrieve(&request("bank", "shared"))
        .expect("retrieve"));
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(fx::FailReranker));
    let out = retriever
        .retrieve(&request("bank", "shared"))
        .expect("retrieve");
    assert!(!out.reranked);
    assert_eq!(ids(&out), fused);
    // Fail-open: no reranker value is written on error.
    for hit in &out.hits {
        assert_eq!(hit.scores.reranker, None, "{hit:?}");
    }
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("rerank unavailable"))
    );
}
#[test]
fn t08_rerank_sidecar_down_keeps_fused_order() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "shared token");
    fx::put_lexical(&store, &entry, "shared token");
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(
        HttpReranker::new(fx::dead_url(), None).expect("client"),
    ));
    let out = retriever
        .retrieve(&request("bank", "shared"))
        .expect("retrieve");
    assert!(!out.reranked);
    assert_eq!(ids(&out), vec!["A".to_owned()]);
    // Dead sidecar: fused order kept, no reranker value written.
    assert_eq!(out.hits[0].scores.reranker, None);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("rerank unavailable"))
    );
}
#[test]
fn a_shortlist_larger_than_the_service_batch_cap_is_still_reranked() {
    const CANDIDATES: usize = 60;
    const CAP: usize = 32;
    let store = fx::store();
    for i in 0..CANDIDATES {
        let entry = fx::item(&format!("item-{i:02}"), "bank", &format!("candidate {i}"));
        fx::put_lexical(&store, &entry, &candidate_text(i));
    }
    let mut req = request("bank", "shared");
    req.limit = CANDIDATES;
    let fused = ids(&fx::retriever(&store).retrieve(&req).expect("retrieve"));
    assert_eq!(fused.len(), CANDIDATES, "full fused shortlist: {fused:?}");

    // The sidecar caps one request at `CAP` documents, below the shortlist.
    let (url, handle) = batch_server(vec![
        reranked_batch(0, CAP, CANDIDATES),
        reranked_batch(CAP, CANDIDATES - CAP, CANDIDATES),
    ]);
    let mut client = HttpReranker::new(url, None).expect("client");
    client.set_max_batch(CAP);
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(client));
    let out = retriever.retrieve(&req).expect("retrieve");
    let requests = handle.join().expect("server join");

    assert!(out.reranked, "the reranker ran: {:?}", out.warnings);
    assert!(
        !out.warnings
            .iter()
            .any(|w| w.contains("rerank unavailable")),
        "{:?}",
        out.warnings
    );
    let sizes: Vec<usize> = requests.iter().map(|r| texts(r).len()).collect();
    assert_eq!(sizes, vec![CAP, CANDIDATES - CAP], "one request per batch");
    // Each batch came back reversed, so the page is the fused order reversed —
    // not the fused order — and every hit keeps the score it was served.
    let mut expected: Vec<String> = fused[..CAP].iter().rev().cloned().collect();
    expected.extend(fused[CAP..].iter().rev().cloned());
    assert_eq!(ids(&out), expected, "ordered by reranker score");
    for (rank, hit) in out.hits.iter().enumerate() {
        let served = (CANDIDATES - 1 - rank) as f64 / 100.0;
        assert_eq!(
            hit.scores.reranker,
            Some(served),
            "hit {rank}: {}",
            hit.hit.item_id
        );
    }
}

#[test]
fn rerank_can_be_cleared() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "shared token");
    fx::put_lexical(&store, &entry, "shared token");
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(fx::ReverseReranker));
    retriever.clear_reranker();
    let out = retriever
        .retrieve(&request("bank", "shared"))
        .expect("retrieve");
    assert!(!out.reranked);
}

/// A `reranker` floor above every score removes the whole shortlist, and the
/// empty page names the floor that did it.
#[test]
fn a_reranker_floor_that_removes_every_hit_names_the_floor() {
    let store = fx::store();
    let entry = fx::item("A", "bank", "shared token");
    fx::put_lexical(&store, &entry, "shared token");
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(fx::ReverseReranker));
    let mut req = request("bank", "shared");
    // `ReverseReranker` scores the only hit 0.9; a floor above it removes it.
    req.min_scores.reranker = Some(0.95);
    let out = retriever.retrieve(&req).expect("retrieve");
    assert!(out.reranked, "the reranker ran: {out:?}");
    assert!(out.hits.is_empty(), "{out:?}");
    assert!(
        out.warnings
            .iter()
            .any(|warning| warning.contains("min_scores.reranker")),
        "the binding floor is named: {:?}",
        out.warnings
    );
}
