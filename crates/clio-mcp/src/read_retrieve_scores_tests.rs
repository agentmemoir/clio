// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP `scores` object tests: binding
//! parity between the MCP payload and the in-process surface for the same
//! store, and the documented `final` / `reranker` / `semantic` / `keyword`
//! key contract.
//!
//! ## Owns
//! - `scores` key/value assertions over the MCP `retrieve` payload
//! - The lexical-only `semantic: null` expectation for the no-embedder state
//! - The populated `semantic` expectation for a resolved dense arm, with
//!   in-process parity for the same store and args
//!
//! ## Does not own
//! - Score computation or the dense transform (`clio-retrieve` stage scoring)
//! - The numbers-or-null security property and the `age_factor` range
//!   (`read_scores_age_factor_tests`)
//! - Transport parity masking (`mcp_read_transport_conformance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the per-hit `scores` contract.

use crate::runtime::{McpOpenOptions, McpState};
use clio_retrieve::retrieve_from_json;
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use super::read_scores_test_support::{base_item, call, hits, put, scores_of, state};

/// The ids of the payload hits, sorted by the caller.
fn hit_ids(payload: &Value) -> Vec<&str> {
    hits(payload)
        .iter()
        .filter_map(|h| h["item_id"].as_str())
        .collect()
}

fn hit_by_id<'a>(payload: &'a Value, id: &str) -> &'a Value {
    hits(payload)
        .iter()
        .find(|h| h["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))
}

/// The fused `final` mirrors the top-level `score` for the same hit.
fn assert_final_mirrors_score(hit: &Value) {
    assert!(hit["score"].is_number(), "numeric score: {hit}");
    assert!(
        hit["scores"]["final"].is_number(),
        "numeric scores.final: {hit}"
    );
    assert_eq!(
        hit["scores"]["final"], hit["score"],
        "scores.final mirrors score: {hit}"
    );
}

/// The MCP payload and the in-process surface expose the same `scores` names
/// and the same raw arm values for the same store. `final` is never compared
/// across the two calls: each request carries no `now`, so the engine reads
/// its own system clock, and the active retention curve multiplies `final` by
/// a smooth age factor taken against that reading. Two calls made at different
/// moments can therefore score different `final` values; the arm scores
/// (`semantic`, `keyword`, `reranker`) are clock-free and are compared.
#[test]
fn mcp_payload_and_in_process_surface_expose_identical_scores() {
    let state = state();
    put(
        &state,
        &base_item("A", "bank-a", "alpha bravo charlie"),
        "alpha bravo charlie",
    );
    put(
        &state,
        &base_item("B", "bank-a", "alpha delta echo"),
        "alpha delta echo",
    );

    let args = json!({"bank": "bank-a", "query": "alpha", "limit": 5});
    let mcp = call(&state, "retrieve", args.clone());
    assert_eq!(mcp["ok"], json!(true), "{mcp}");

    let in_process = serde_json::to_value(retrieve_from_json(&state.retriever, &args))
        .expect("serialize in-process outcome");
    assert_eq!(in_process["ok"], json!(true), "{in_process}");

    let mut mcp_ids = hit_ids(&mcp);
    let mut in_process_ids = hit_ids(&in_process);
    mcp_ids.sort_unstable();
    in_process_ids.sort_unstable();
    assert_eq!(
        mcp_ids, in_process_ids,
        "same store, same hits: {mcp} vs {in_process}"
    );
    assert!(!mcp_ids.is_empty(), "the query matched seeded items: {mcp}");

    for id in mcp_ids {
        let mcp_hit = hit_by_id(&mcp, id);
        let in_process_hit = hit_by_id(&in_process, id);
        let mcp_scores = scores_of(mcp_hit);
        let in_process_scores = scores_of(in_process_hit);
        assert_final_mirrors_score(mcp_hit);
        assert_final_mirrors_score(in_process_hit);
        assert_eq!(mcp_scores["reranker"], json!(null), "{mcp_hit}");
        assert_eq!(
            in_process_scores["reranker"],
            json!(null),
            "{in_process_hit}"
        );
        assert_eq!(
            mcp_scores["semantic"], in_process_scores["semantic"],
            "semantic parity for `{id}`"
        );
        assert_eq!(
            mcp_scores["keyword"], in_process_scores["keyword"],
            "keyword parity for `{id}`"
        );
    }
}

/// A hit scored only by the lexical arm reports `semantic: null` (never a
/// fabricated `0`) and the backend's raw numeric `keyword`.
#[test]
fn lexical_only_hits_report_null_semantic_and_numeric_keyword() {
    let state = state();
    assert!(
        !state.pipeline.dense_available,
        "no embedder configured: dense arm is off"
    );
    put(
        &state,
        &base_item("L", "bank-a", "gamma delta epsilon"),
        "gamma delta epsilon",
    );

    let out = call(
        &state,
        "retrieve",
        json!({"bank": "bank-a", "query": "gamma"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let hit = hit_by_id(&out, "L");
    let scores = scores_of(hit);
    assert_eq!(
        scores["semantic"],
        json!(null),
        "absent dense arm is null, not 0: {hit}"
    );
    assert!(
        scores["keyword"].is_number(),
        "the lexical arm scored the hit: {hit}"
    );
    assert_eq!(scores["reranker"], json!(null), "{hit}");
    assert_final_mirrors_score(hit);
    assert!(
        hit.get("dense_rank").is_none(),
        "the dense arm did not rank it: {hit}"
    );
    assert!(hit["lexical_rank"].is_number(), "{hit}");
}

/// Dense embedding width for the fake TEI double (matches the effective config).
const DENSE_DIMS: usize = 384;

/// A 384-dim unit vector on the first axis. Every embed request gets the same
/// vector, so a stored item and its query sit at cosine similarity `1.0`.
fn unit_vector() -> Vec<f32> {
    let mut vector = vec![0.0_f32; DENSE_DIMS];
    vector[0] = 1.0;
    vector
}

fn http_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A local TEI double that answers `POST /embed` with one unit vector.
///
/// Two hardening rules from issue #31: accepted streams inherit the
/// listener's nonblocking mode, so force blocking before the read timeout;
/// and loop reads until the request line is complete, since a partial
/// segment never starts with `POST /embed` and misroutes to the `{}` arm.
/// Read timeouts plus a wall-clock deadline keep a missing request from
/// hanging the test. The thread stays detached (never `join`ed).
fn fake_tei_server(expected_requests: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake TEI server");
    let port = listener.local_addr().expect("fake TEI address").port();
    listener
        .set_nonblocking(true)
        .expect("nonblocking fake TEI listener");
    let _server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut served = 0;
        while served < expected_requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut request = [0_u8; 16 * 1024];
                    let mut filled = 0;
                    while filled < request.len()
                        && !request[..filled].contains(&b'\n')
                        && !request[..filled].contains(&b'\r')
                    {
                        match stream.read(&mut request[filled..]) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => filled += n,
                        }
                    }
                    let request = &request[..filled];
                    let request_line = request
                        .split(|byte| *byte == b'\r')
                        .next()
                        .unwrap_or(request);
                    let body = if request_line.starts_with(b"POST /embed") {
                        served += 1;
                        json!([unit_vector()]).to_string()
                    } else {
                        "{}".to_owned()
                    };
                    let response = http_response(&body);
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
                Err(ref error)
                    if error.kind() == ErrorKind::WouldBlock
                        || error.kind() == ErrorKind::Interrupted =>
                {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break,
            }
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// A verified, admitted store payload that leaves a durable outbox row. The
/// gist doubles as the query text so the dense leg scores it at similarity `1.0`.
fn durable_store_args(id: &str, gist: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": id,
            "gist": gist,
            "snapshot": {"entity": gist, "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": format!("{gist} (1 note since 2024-01-01)."),
        "bank": "bank-a",
        "actor": "agent"
    })
}

/// Dense parity: with an embedder resolved and its outbox drained,
/// the MCP `retrieve` payload carries a POPULATED `semantic` (a number in
/// `[0, 1]`, never null) that matches the in-process surface for the same store
/// and args.
#[test]
fn dense_arm_populates_semantic_in_mcp_payload_with_parity() {
    // The drain, the MCP retrieve, and the in-process retrieve each embed the
    // query once, so the double serves three requests.
    let tei_url = fake_tei_server(3);
    let effective = json!({
        "embed": {
            "url": tei_url,
            "model": "BAAI/bge-small-en-v1.5",
            "dims": DENSE_DIMS,
            "provider": "tei"
        }
    });
    let state = McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: "bank-a".into(),
            ..McpOpenOptions::default()
        },
        &effective,
    )
    .expect("state");
    assert!(
        state.pipeline.dense_available,
        "the embed block resolves a dense arm"
    );

    // Store one item through the dispatcher so a durable outbox row exists; the
    // `put` helper writes rows directly and would leave nothing to drain.
    let gist = "alpha bravo charlie";
    let stored = call(&state, "store", durable_store_args("D", gist));
    assert_eq!(stored["ok"], json!(true), "{stored}");
    assert_eq!(stored["id"], json!("D"));

    let report = state.drain_index().expect("drain index");
    assert_eq!(report.applied, 1, "one stored vector: {report:?}");
    assert_eq!(report.failed, 0, "{report:?}");
    assert_eq!(report.lexical_only, 0, "{report:?}");

    let args = json!({"bank": "bank-a", "query": gist, "limit": 5});
    let mcp = call(&state, "retrieve", args.clone());
    assert_eq!(mcp["ok"], json!(true), "{mcp}");

    let hit = hit_by_id(&mcp, "D");
    assert!(
        hit["dense_rank"].is_number(),
        "the dense leg ranked the hit: {hit}"
    );
    let scores = scores_of(hit);
    let semantic = scores["semantic"]
        .as_f64()
        .unwrap_or_else(|| panic!("populated semantic number, not null: {hit}"));
    assert!(
        (0.0..=1.0).contains(&semantic),
        "semantic is a similarity in [0, 1]: {hit}"
    );
    assert!(scores["keyword"].is_number(), "{hit}");
    assert_eq!(scores["reranker"], json!(null), "{hit}");
    assert_final_mirrors_score(hit);

    // In-process parity: the same store and args report the same `semantic`.
    let in_process = serde_json::to_value(retrieve_from_json(&state.retriever, &args))
        .expect("serialize in-process outcome");
    assert_eq!(in_process["ok"], json!(true), "{in_process}");
    let in_process_hit = hit_by_id(&in_process, "D");
    assert!(in_process_hit["dense_rank"].is_number(), "{in_process_hit}");
    let in_process_scores = scores_of(in_process_hit);
    assert_eq!(
        in_process_scores["semantic"], scores["semantic"],
        "dense semantic parity for `D`"
    );
}
