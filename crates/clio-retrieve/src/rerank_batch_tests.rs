// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the request-batching behavior of the TEI
//! sidecar rerank adapter: splitting a shortlist into requests that fit the
//! service batch ceiling and merging the scored parts back into one
//! index-consistent result.
//!
//! ## Owns
//! - The documents-per-request split, the per-chunk wire body, the global index
//!   offset applied to each merged entry, and the global best-first merge order
//! - The configured cap the factory reads from the document, and the default it
//!   keeps when that key is absent
//!
//! ## Does not own
//! - Response parsing rules (`rerank_parse`) or the single-request wire shape
//!   and provider selection (`rerank_tests`)
//! - The fake rerank service and request decoder (`rerank_seam`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::rerank::rerank_tests::indices;
use crate::rerank::{DEFAULT_RERANK_MAX_BATCH, HttpReranker, Reranker, build_reranker_from};
use crate::rerank_seam::{batch_server, json_body, texts};
use serde_json::{Value, json};

fn docs(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("doc-{i}")).collect()
}

fn http_ok(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn http_unprocessable() -> String {
    let body = "{}";
    format!(
        "HTTP/1.1 422 Unprocessable Entity\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A TEI-shaped success body for one chunk: entry `i` belongs to global
/// document `start + i`, and its score encodes that global index so a missing
/// offset shift cannot pass unnoticed.
fn chunk_body(start: usize, len: usize, reverse: bool) -> Value {
    let mut locals: Vec<usize> = (0..len).collect();
    if reverse {
        locals.reverse();
    }
    Value::Array(
        locals
            .into_iter()
            .map(|i| json!({"index": i, "score": (start + i) as f64 / 100.0}))
            .collect(),
    )
}

#[test]
fn sixty_document_shortlist_splits_into_two_capped_requests() {
    assert_eq!(DEFAULT_RERANK_MAX_BATCH, 32);
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 32, false)),
        http_ok(&chunk_body(32, 28, false)),
    ]);
    let client = HttpReranker::new(url, None).expect("build");
    client
        .rerank("needle", &docs(60), 60)
        .expect("batched rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 2, "one request per batch");
    for request in &captured {
        assert!(request.starts_with("POST /rerank "), "{request}");
        assert_eq!(json_body(request)["raw_scores"], json!(false));
    }
    assert_eq!(texts(&captured[0]), docs(60)[..32].to_vec());
    assert_eq!(texts(&captured[1]), docs(60)[32..].to_vec());
}

#[test]
fn merged_result_carries_global_indices_and_their_own_scores() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 32, false)),
        http_ok(&chunk_body(32, 28, false)),
    ]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(60), 60).expect("rerank");
    handle.join().expect("server join");

    assert_eq!(merged.len(), 60, "every document scored");
    // The served score rises with the global index, so a best-first result is
    // the global order reversed — and every document is still present once.
    assert_eq!(
        indices(&merged),
        (0..60_usize).rev().collect::<Vec<usize>>()
    );
    for doc in &merged {
        // The score encodes the global index, so a batch without its offset
        // shift reports the second chunk as 0..27 and fails here.
        assert_eq!(
            doc.score,
            Some(doc.index as f64 / 100.0),
            "score/index mismatch at {}",
            doc.index
        );
    }
}

#[test]
fn shortlist_within_the_cap_makes_one_request() {
    let (url, handle) = batch_server(vec![http_ok(&chunk_body(0, 30, false))]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(30), 30).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 1);
    assert_eq!(texts(&captured[0]), docs(30));
    assert_eq!(
        indices(&merged),
        (0..30_usize).rev().collect::<Vec<usize>>()
    );
}

/// A shortlist exactly at the cap fits one request: the boundary the split
/// depends on, where one document too many would open a second connection.
#[test]
fn shortlist_exactly_at_the_cap_makes_one_request() {
    let (url, handle) = batch_server(vec![http_ok(&chunk_body(0, 32, false))]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(32), 32).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 1, "32 documents over a cap of 32");
    assert_eq!(texts(&captured[0]).len(), 32);
    assert_eq!(merged.len(), 32);
    assert_eq!(
        indices(&merged),
        (0..32_usize).rev().collect::<Vec<usize>>()
    );
}

/// A one-document shortlist is a single request and a single entry.
#[test]
fn single_document_shortlist_reranks_in_one_request() {
    let (url, handle) = batch_server(vec![http_ok(&chunk_body(0, 1, false))]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(1), 1).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 1);
    assert_eq!(texts(&captured[0]), vec!["doc-0".to_owned()]);
    assert_eq!(indices(&merged), vec![0]);
    assert_eq!(merged[0].score, Some(0.0));
}

#[test]
fn failing_second_chunk_returns_the_http_error_and_no_partial_result() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 32, false)),
        http_unprocessable(),
    ]);
    let client = HttpReranker::new(url, None).expect("build");
    let err = client
        .rerank("needle", &docs(60), 60)
        .expect_err("second chunk fails");
    handle.join().expect("server join");

    assert!(err.message.contains("HTTP 422"), "{err}");
    // The failure names the chunk that broke and the documents it carried, so
    // a second-chunk rejection is distinguishable from a dead sidecar.
    assert!(
        err.message
            .starts_with("rerank chunk 2/2 (docs 32..60) failed:"),
        "{err}"
    );
}

/// A chunk-major merge would put every first-chunk document ahead of the
/// second chunk, so the top-scored document lives in chunk 1 here: only a
/// globally best-first merge can return it first.
#[test]
fn merged_result_is_best_first_across_the_chunk_boundary() {
    let low = Value::Array(
        (0..32)
            .map(|i| json!({"index": i, "score": 0.10 + f64::from(i) / 1000.0}))
            .collect(),
    );
    let high = Value::Array(
        (0..28)
            .map(|i| json!({"index": i, "score": 0.50 + f64::from(i) / 1000.0}))
            .collect(),
    );
    let (url, handle) = batch_server(vec![http_ok(&low), http_ok(&high)]);
    let client = HttpReranker::new(url, None).expect("build");
    let merged = client.rerank("needle", &docs(60), 60).expect("rerank");
    handle.join().expect("server join");

    // Every chunk-1 document outranks every chunk-0 document, and each chunk
    // was served descending, so best-first is chunk 1 reversed then chunk 0.
    let expected: Vec<usize> = (32..60_usize).rev().chain((0..32_usize).rev()).collect();
    assert_eq!(indices(&merged), expected);
    assert_eq!(merged[0].index, 59, "the highest score wins the merge");
    // The returned order never inverts the score order.
    for pair in merged.windows(2) {
        let (outer, inner) = (
            pair[0].score.expect("scored"),
            pair[1].score.expect("scored"),
        );
        assert!(outer >= inner, "merge is not best-first: {merged:?}");
    }
}

#[test]
fn zero_max_batch_is_clamped_to_one_document_per_request() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 1, false)),
        http_ok(&chunk_body(1, 1, false)),
    ]);
    let mut client = HttpReranker::new(url, None).expect("build");
    client.set_max_batch(0);
    let merged = client.rerank("needle", &docs(2), 2).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 2);
    assert_eq!(texts(&captured[0]), vec!["doc-0".to_owned()]);
    assert_eq!(texts(&captured[1]), vec!["doc-1".to_owned()]);
    // doc-1 was served the higher score, so it leads the merged result.
    assert_eq!(indices(&merged), vec![1, 0]);
}

/// The cap an operator configures reaches the built adapter, so a sidecar with
/// its own smaller ceiling can be matched from the config document.
#[test]
fn factory_applies_the_configured_batch_cap() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, 7, false)),
        http_ok(&chunk_body(7, 2, false)),
    ]);
    let document = json!({"rerank": {"url": url, "model": "m", "max_batch": 7}});
    let reranker = build_reranker_from(&document, None)
        .expect("build")
        .expect("a configured url builds a reranker");
    let merged = reranker.rerank("needle", &docs(9), 9).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 2, "nine documents over a cap of seven");
    assert_eq!(texts(&captured[0]), docs(9)[..7].to_vec());
    assert_eq!(texts(&captured[1]), docs(9)[7..].to_vec());
    assert_eq!(indices(&merged), (0..9_usize).rev().collect::<Vec<usize>>());
}

/// A document that does not name a cap keeps the adapter's own default.
#[test]
fn factory_without_a_configured_cap_keeps_the_default() {
    let (url, handle) = batch_server(vec![
        http_ok(&chunk_body(0, DEFAULT_RERANK_MAX_BATCH, false)),
        http_ok(&chunk_body(DEFAULT_RERANK_MAX_BATCH, 28, false)),
    ]);
    let document = json!({"rerank": {"url": url, "model": "m"}});
    let reranker = build_reranker_from(&document, None)
        .expect("build")
        .expect("a configured url builds a reranker");
    let merged = reranker.rerank("needle", &docs(60), 60).expect("rerank");
    let captured = handle.join().expect("server join");

    assert_eq!(captured.len(), 2);
    assert_eq!(texts(&captured[0]).len(), DEFAULT_RERANK_MAX_BATCH);
    assert_eq!(
        texts(&captured[1]),
        docs(60)[DEFAULT_RERANK_MAX_BATCH..].to_vec()
    );
    assert_eq!(
        indices(&merged),
        (0..60_usize).rev().collect::<Vec<usize>>()
    );
}
