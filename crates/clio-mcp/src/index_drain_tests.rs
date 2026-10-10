// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the runtime index-drain surface:
//! outbox drain to retrieval without manual seeding, lexical-only fallback,
//! dense-failure retry accounting, rerank attach, the non-blocking write path,
//! Cadence, bounded backlog, raw-ingest to retrieval, and pipeline
//! status reporting.
//!
//! ## Owns
//! - In-memory `McpState` scaffolding and one-shot fake HTTP servers
//! - The scenario tests for drain, nudge, and sweeper behavior
//!
//! ## Does not own
//! - Production drain/sweeper behavior (`index_drain` / `runtime_index`)
//! - Outbox processing policy (`clio-index`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[allow(clippy::needless_pass_by_value)]
fn state_with(effective: Value) -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &effective).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// A verified, admitted store payload (span-grounded entity + number + date).
fn good_item_args(id: &str, gist: &str) -> Value {
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

/// One-shot fake HTTP server returning a canned raw response.
fn fake_http_server(response: String) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0_u8; 8192];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

fn http_ok(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn hit_ids(out: &Value) -> Vec<String> {
    out["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .filter_map(|h| h["item_id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// store → drain → retrieve with no manual
/// seeding, using the lexical-only fallback (no embedder configured).
#[test]
fn t35_01_store_drain_retrieve_lexical_only() {
    let state = state_with(json!({}));
    assert!(!state.pipeline.dense_available, "no embedder configured");
    let stored = call(&state, "store", good_item_args("itm-1", "terse answers"));
    assert_eq!(stored["ok"], json!(true), "{stored}");
    assert_eq!(stored["id"], json!("itm-1"));

    // The write path left a durable job; draining is what makes it searchable.
    let before = state.index_status_json("bank-a").expect("status");
    assert_eq!(before["pending_jobs"], json!(1));
    assert_eq!(before["lexical_rows"], json!(0));

    let report = state.drain_index().expect("drain");
    assert_eq!(report.lexical_only, 1, "{report:?}");
    assert_eq!(report.applied, 0);
    assert_eq!(report.failed, 0);

    let out = call(
        &state,
        "retrieve",
        json!({"query": "terse answers", "bank": "bank-a", "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(hit_ids(&out).contains(&"itm-1".to_owned()), "{out}");

    let after = state.index_status_json("bank-a").expect("status");
    assert_eq!(after["pending_jobs"], json!(0));
    assert_eq!(after["lexical_rows"], json!(1));
    assert_eq!(after["dense_rows"], json!(0));
    assert_eq!(after["dense_available"], json!(false));
    assert_eq!(after["model_id"], json!(""));
    assert_eq!(after["rerank_active"], json!(false));
    assert_eq!(after["extract_available"], json!(false));
}

/// A failing embedder still writes the lexical doc, records the
/// failure, and leaves the job pending for retry (never falsely finished).
#[test]
fn t35_03_and_10_dense_failure_is_recorded_and_retried() {
    let state = state_with(json!({
        "embed": {"url": "http://127.0.0.1:1", "model": "BAAI/bge-small-en-v1.5", "dims": 384, "provider": "tei"}
    }));
    assert!(state.pipeline.dense_available);
    let stored = call(&state, "store", good_item_args("itm-1", "dense fails here"));
    assert_eq!(stored["ok"], json!(true), "{stored}");

    let report = state.drain_index().expect("drain");
    assert_eq!(report.failed, 1, "{report:?}");
    assert_eq!(report.applied, 0);

    // Lexical survived the dense failure; the job is still queued.
    let status = state.index_status_json("bank-a").expect("status");
    assert_eq!(status["lexical_rows"], json!(1));
    assert_eq!(status["dense_rows"], json!(0));
    assert_eq!(status["pending_jobs"], json!(1));
    assert_eq!(status["retry_jobs"], json!(1));
    assert!(
        status["coverage_last_error"].as_str().is_some(),
        "failure must be surfaced: {status}"
    );

    // Bounded retry: an immediate second sweep sits out the backoff window.
    let again = state.drain_index().expect("drain");
    assert_eq!(again.failed, 0, "retry honors backoff");
    assert_eq!(again.applied, 0);
}

/// A configured reranker is attached and applies; unset is unchanged.
#[test]
fn t35_04_and_05_rerank_attach_decision() {
    // Baseline: rerank unset (default effective config).
    let unset = state_with(json!({}));
    assert!(!unset.pipeline.rerank_active);
    // Distinct gists per id: a shared one is an exact duplicate, and the
    // unconditional write-side rule refuses the second store.
    for (id, gist) in [("itm-a", "alpha beta itm-a"), ("itm-b", "alpha beta itm-b")] {
        assert_eq!(
            call(&unset, "store", good_item_args(id, gist))["ok"],
            json!(true)
        );
    }
    unset.drain_index().expect("drain");
    let base = call(
        &unset,
        "retrieve",
        json!({"query": "alpha beta", "bank": "bank-a", "limit": 5}),
    );
    assert_eq!(base["reranked"], json!(false), "{base}");
    let baseline = hit_ids(&base);
    assert!(baseline.len() >= 2, "{base}");

    // Configured: a mock sidecar that reverses the shortlist and scores it.
    let (url, handle) = fake_http_server(http_ok(&json!({"results": [
        {"index": 1, "relevance_score": 0.9},
        {"index": 0, "relevance_score": 0.3}
    ]})));
    let state = state_with(json!({"rerank": {"url": url, "model": "m", "provider": "tei"}}));
    assert!(state.pipeline.rerank_active);
    // Distinct gists per id: a shared one is an exact duplicate, and the
    // unconditional write-side rule refuses the second store.
    for (id, gist) in [("itm-a", "alpha beta itm-a"), ("itm-b", "alpha beta itm-b")] {
        assert_eq!(
            call(&state, "store", good_item_args(id, gist))["ok"],
            json!(true)
        );
    }
    state.drain_index().expect("drain");
    let out = call(
        &state,
        "retrieve",
        json!({"query": "alpha beta", "bank": "bank-a", "limit": 5}),
    );
    assert_eq!(out["reranked"], json!(true), "{out}");
    let mut reordered = hit_ids(&out);
    assert_eq!(reordered.len(), baseline.len(), "{out}");
    reordered.sort();
    let mut expected = baseline;
    expected.sort();
    assert_eq!(reordered, expected, "rerank must keep the same hit set");
    // The normalized provider relevance reaches the hit's `scores` object.
    for hit in out["hits"].as_array().expect("hits") {
        let reranker = &hit["scores"]["reranker"];
        assert!(
            reranker.is_number() && (0.0..=1.0).contains(&reranker.as_f64().expect("f64")),
            "{hit}"
        );
    }
    // No silent failures while rerank is live.
    let warnings = out["warnings"].as_array().expect("warnings");
    assert!(warnings.is_empty(), "{warnings:?}");
    handle.join().expect("fake server");
}

/// The store response never embeds or drains on the request path.
#[test]
fn t35_09_write_response_is_not_blocked_by_embedding() {
    let state = state_with(json!({
        "embed": {"url": "http://127.0.0.1:1", "model": "BAAI/bge-small-en-v1.5", "dims": 384, "provider": "tei"}
    }));
    let started = Instant::now();
    for i in 0..3 {
        let out = call(
            &state,
            "store",
            good_item_args(&format!("itm-{i}"), &format!("slow leg itm-{i}")),
        );
        assert_eq!(out["ok"], json!(true), "{out}");
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "store must not wait on embedding: {elapsed:?}"
    );
    // The jobs are still queued: nothing embedded on the write path.
    assert_eq!(
        state.index_status_json("bank-a").expect("status")["pending_jobs"],
        json!(3)
    );
}

/// With the sweeper running, a stored item is retrievable
/// within the window (post-write nudge, no manual drain).
#[test]
fn t35_13_stored_item_retrievable_within_nfr2() {
    let state = state_with(json!({}));
    let sweeper = state.spawn_index_sweeper_with(Duration::from_millis(5));
    let stored = call(&state, "store", good_item_args("itm-1", "nfr two answers"));
    assert_eq!(stored["ok"], json!(true), "{stored}");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut found = false;
    while Instant::now() < deadline {
        let out = call(
            &state,
            "retrieve",
            json!({"query": "nfr two answers", "bank": "bank-a", "limit": 5}),
        );
        if hit_ids(&out).contains(&"itm-1".to_owned()) {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        found,
        "item must become retrievable within 2s via the nudge"
    );
    drop(state);
    sweeper.join().expect("sweeper exits when state drops");
}

/// A backlog larger than one batch is visible and drains in
/// bounded batches instead of growing without bound.
#[test]
fn t35_14_backlog_is_visible_and_drains_in_bounded_batches() {
    let state = state_with(json!({}));
    let bank = "bank-a";
    let count = state.index_drain.batch_size() + 1;
    for i in 0..count {
        let item = clio_types::MemoryItem {
            id: format!("bl-{i}"),
            bank_id: bank.into(),
            subject_id: format!("bl-{i}"),
            kind: clio_types::ItemKind::Semantic,
            category: Some(clio_types::SemanticCategory::Schema),
            episodic_type: None,
            epistemic_kind: clio_types::EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            admission_score: None,
            source_ref: None,
            snapshot: Some(json!({"entity": "backlog"})),
            gist: Some(format!("backlog item {i}")),
            valid_from: "t0".into(),
            valid_until: None,
            tx_from: "t0".into(),
            tx_until: None,
            created_at: "t0".into(),
            updated_at: "t0".into(),
            context: None,
        };
        state
            .store
            .create_memory_item(&item, "agent")
            .expect("seed");
    }
    assert_eq!(
        state.index_status_json(bank).expect("status")["pending_jobs"],
        json!(count)
    );
    let first = state.drain_index().expect("drain");
    assert_eq!(first.lexical_only as usize, count - 1, "{first:?}");
    let mid = state.index_status_json(bank).expect("status");
    assert_eq!(mid["pending_jobs"], json!(1));
    let second = state.drain_index().expect("drain");
    assert_eq!(second.lexical_only, 1, "{second:?}");
    assert_eq!(
        state.index_status_json(bank).expect("status")["pending_jobs"],
        json!(0)
    );
}

/// `maintenance_status` surfaces the live pipeline state.
#[test]
fn t35_maintenance_status_reports_index_pipeline() {
    let state = state_with(json!({}));
    assert_eq!(
        call(&state, "store", good_item_args("itm-1", "status answers"))["ok"],
        json!(true)
    );
    let out = call(&state, "maintenance_status", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    let index = &out["index"];
    assert_eq!(index["dense_available"], json!(false));
    assert_eq!(index["rerank_active"], json!(false));
    assert_eq!(index["extract_available"], json!(false));
    assert_eq!(index["pending_jobs"], json!(1));
    assert_eq!(index["index_mode"], json!("ann_vector"));
}

/// The periodic sweep drains jobs with no post-write nudge (retry/backoff and
/// non-`store` write paths ride this path).
#[test]
fn t35_periodic_sweep_drains_without_a_nudge() {
    let state = state_with(json!({}));
    let bank = "bank-a";
    let item = clio_types::MemoryItem {
        id: "periodic-1".into(),
        bank_id: bank.into(),
        subject_id: "periodic-1".into(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": "periodic"})),
        gist: Some("periodic sweep answers".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    };
    state
        .store
        .create_memory_item(&item, "agent")
        .expect("seed");
    assert_eq!(
        state.index_status_json(bank).expect("status")["pending_jobs"],
        json!(1)
    );

    let sweeper = state.spawn_index_sweeper_with(Duration::from_millis(5));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut drained = false;
    while Instant::now() < deadline {
        if state.index_status_json(bank).expect("status")["pending_jobs"] == json!(0) {
            drained = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(drained, "the periodic sweep must drain without a nudge");
    drop(state);
    sweeper.join().expect("sweeper exits when state drops");
}

/// library raw ingest feeds the same durable outbox, so an
/// ingested turn becomes retrievable end to end with no manual seeding.
#[test]
fn t35_06_raw_ingest_drains_and_retrieves_end_to_end() {
    let body = json!({"snapshot": {"entity": "Ada"}, "gist": "ada likes tea", "source_ref": "s"});
    let (url, handle) = fake_http_server(http_ok(&body));
    let state = state_with(json!({}));
    let ctx = clio_types::OpContext::new(clio_types::Bank::new("bank-a"), clio_types::Actor::Agent);
    let turns = vec![clio_write::Turn::text("Ada likes tea")];
    let signals = clio_write::EmptySignals;
    let opts = clio_write::RawIngestOptions {
        context: None,
        store: &*state.store,
        ctx: &ctx,
        turns: &turns,
        timestamp: "2026-01-01T00:00:00Z",
        item_id_prefix: "raw",
        candidate: clio_admission::decision::Candidate::Episodic(
            clio_types::EpisodicType::Temporal,
        ),
        signal_source: &signals,
        schema_template: json!({"entity": ""}),
        config: clio_write::ingest::WriteIngestConfig::default(),
        leaves: &state.leaves,
        maintenance: Arc::new(clio_write::NoopMaintenance),
    };
    let effective = json!({"extract": {"provider": "template", "url": url}});
    let out = clio_write::ingest_raw(&opts, &effective).expect("ingest");
    handle.join().expect("fake extract server");
    assert_eq!(out.admitted.len(), 1, "{out:?}");
    let id = out.admitted[0].item_id.clone();
    let report = state.drain_index().expect("drain");
    assert_eq!(report.lexical_only, 1, "{report:?}");
    assert_eq!(report.failed, 0);
    let retrieved = call(
        &state,
        "retrieve",
        json!({"query": "ada likes tea", "bank": "bank-a", "limit": 5}),
    );
    assert_eq!(retrieved["ok"], json!(true), "{retrieved}");
    assert!(hit_ids(&retrieved).contains(&id), "{retrieved}");
}
