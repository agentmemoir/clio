// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the raw-ingest entry points:
//! grounded admit + store + index enqueue, ungrounded refusal, the
//! unavailable-extraction guard, the config-resolved HTTP extractor path,
//! and the empty-input edge case.
//!
//! ## Owns
//! - In-memory store scaffolding and a one-shot fake HTTP extraction server
//! - A context-recording fixture wrapper for context pass-through assertions
//!
//! ## Does not own
//! - Production raw-ingest behavior (see `raw_ingest`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::chunk::Turn;
use crate::extract::{
    ExtractCandidate, ExtractRequest, Extractor, FixtureExtractor, PoisonExtractor,
};
use crate::ingest::WriteIngestConfig;
use crate::maintenance::{LeafIndex, NoopMaintenance};
use crate::raw_ingest::{RawIngestOptions, ingest_raw, ingest_raw_with_extractor};
use clio_admission::decision::Candidate;
use clio_store::{IndexStore, LocalDevKms, SqliteStore, Store};
use clio_types::{Actor, Bank, CONTEXT_MAX_BYTES, EpisodicType, ErrorCode, OpContext};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const TS: &str = "2026-01-01T00:00:00Z";

static EMPTY_SIGNALS: crate::EmptySignals = crate::EmptySignals;

fn in_memory_store() -> Arc<SqliteStore> {
    Arc::new(SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("open sqlite"))
}

fn options<'a>(
    store: &'a dyn Store,
    ctx: &'a OpContext,
    turns: &'a [Turn],
    leaves: &'a LeafIndex,
    context: Option<&str>,
) -> RawIngestOptions<'a> {
    RawIngestOptions {
        store,
        ctx,
        turns,
        timestamp: TS,
        item_id_prefix: "raw",
        candidate: Candidate::Episodic(EpisodicType::Temporal),
        signal_source: &EMPTY_SIGNALS,
        schema_template: json!({"entity": ""}),
        context: context.map(str::to_owned),
        config: WriteIngestConfig::default(),
        leaves,
        maintenance: Arc::new(NoopMaintenance),
    }
}

/// Fixture extractor that records the context each request carried.
#[derive(Debug)]
struct RecordingFixture {
    fixture: FixtureExtractor,
    contexts: Mutex<Vec<Option<String>>>,
}

impl Extractor for RecordingFixture {
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, clio_types::AmError> {
        self.contexts.lock().unwrap().push(req.context.clone());
        self.fixture.extract(req)
    }
}

fn fixture(source: &str, entity: &str) -> FixtureExtractor {
    let mut fx = FixtureExtractor::new();
    fx.insert(
        source,
        ExtractCandidate {
            snapshot: Some(json!({"entity": entity})),
            gist: Some("g".into()),
            source_ref: "s".into(),
            diagnostic: None,
        },
    );
    fx
}

#[test]
fn grounded_entity_is_admitted_stored_and_enqueued() {
    let store = in_memory_store();
    let fx = fixture("Ada likes tea", "Ada");
    let turns = vec![Turn::text("Ada likes tea")];
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let opts = options(&*store, &ctx, &turns, &leaves, None);

    let out = ingest_raw_with_extractor(&opts, &fx).expect("ingest");
    assert_eq!(out.admitted.len(), 1, "{out:?}");
    let id = out.admitted[0].item_id.clone();
    let stored = store
        .get_memory_item("bank-1", &id)
        .expect("get")
        .expect("stored");
    assert_eq!(stored.snapshot, Some(json!({"entity": "Ada"})));
    let jobs = store.pending_index_jobs(10, None, u32::MAX).expect("jobs");
    assert!(jobs.iter().any(|j| j.item_id == id), "{jobs:?}");
}

#[test]
fn paraphrase_only_candidate_is_refused_with_no_write() {
    let store = in_memory_store();
    let fx = fixture("Ada likes tea", "Bob");
    let turns = vec![Turn::text("Ada likes tea")];
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let opts = options(&*store, &ctx, &turns, &leaves, None);

    let out = ingest_raw_with_extractor(&opts, &fx).expect("ingest");
    assert!(out.admitted.is_empty(), "{out:?}");
    assert!(
        store
            .get_memory_item("bank-1", "raw-0-0")
            .expect("get")
            .is_none()
    );
    let jobs = store.pending_index_jobs(10, None, u32::MAX).expect("jobs");
    assert!(jobs.is_empty(), "index work: {jobs:?}");
}

#[test]
fn missing_extract_url_reports_unavailable_and_writes_nothing() {
    let store = in_memory_store();
    let turns = vec![Turn::text("Ada likes tea")];
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let opts = options(&*store, &ctx, &turns, &leaves, None);

    let err = ingest_raw(&opts, &json!({})).expect_err("unavailable");
    assert_eq!(err.code, ErrorCode::NotImplemented);
    assert!(err.message.contains("extraction unavailable"), "{err}");
    let jobs = store.pending_index_jobs(10, None, u32::MAX).expect("jobs");
    assert!(jobs.is_empty(), "index work: {jobs:?}");
}

#[test]
fn config_resolved_extractor_runs_over_http() {
    let store = in_memory_store();
    let body = json!({"snapshot": {"entity": "Ada"}, "gist": "g", "source_ref": "s"});
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (url, handle) = fake_http_server(response);
    let turns = vec![Turn::text("Ada likes tea")];
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let opts = options(&*store, &ctx, &turns, &leaves, None);

    let effective = json!({"extract": {"provider": "template", "url": url}});
    let out = ingest_raw(&opts, &effective).expect("ingest");
    handle.join().expect("server");
    assert_eq!(out.admitted.len(), 1, "{out:?}");
    let id = &out.admitted[0].item_id;
    assert!(store.get_memory_item("bank-1", id).expect("get").is_some());
}

#[test]
fn supplied_context_reaches_extraction_and_is_sealed_into_stored_items() {
    let store = in_memory_store();
    let fx = RecordingFixture {
        fixture: fixture("Ada likes tea", "Ada"),
        contexts: Mutex::new(vec![]),
    };
    let turns = vec![Turn::text("Ada likes tea")];
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let opts = options(&*store, &ctx, &turns, &leaves, Some("team chat"));

    let out = ingest_raw_with_extractor(&opts, &fx).expect("ingest");
    assert_eq!(out.admitted.len(), 1, "{out:?}");
    // Every extraction request carried the caller's context.
    assert_eq!(
        fx.contexts.lock().unwrap().as_slice(),
        [Some("team chat".to_owned())]
    );
    let id = out.admitted[0].item_id.clone();
    let stored = store
        .get_memory_item("bank-1", &id)
        .expect("get")
        .expect("stored");
    // The admitted item retains the original context after the full pipeline.
    assert_eq!(stored.context.as_deref(), Some("team chat"));
    assert_eq!(stored.snapshot, Some(json!({"entity": "Ada"})));
}

#[test]
fn empty_context_is_rejected_before_extraction() {
    let store = in_memory_store();
    let poison = PoisonExtractor::new("never-called");
    let turns = vec![Turn::text("Ada likes tea")];
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let opts = options(&*store, &ctx, &turns, &leaves, Some(""));

    let err = ingest_raw_with_extractor(&opts, &poison).expect_err("empty context");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    assert_eq!(poison.call_count(), 0, "no extraction may run");
    let jobs = store.pending_index_jobs(10, None, u32::MAX).expect("jobs");
    assert!(jobs.is_empty(), "index work: {jobs:?}");
}

#[test]
fn oversized_context_is_rejected_before_extraction() {
    let store = in_memory_store();
    let poison = PoisonExtractor::new("never-called");
    let turns = vec![Turn::text("Ada likes tea")];
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let oversized = "a".repeat(CONTEXT_MAX_BYTES + 1);
    let opts = options(&*store, &ctx, &turns, &leaves, Some(&oversized));

    let err = ingest_raw_with_extractor(&opts, &poison).expect_err("oversized context");
    assert_eq!(err.code, ErrorCode::OutOfRange, "{err}");
    assert_eq!(poison.call_count(), 0, "no extraction may run");
    let jobs = store.pending_index_jobs(10, None, u32::MAX).expect("jobs");
    assert!(jobs.is_empty(), "index work: {jobs:?}");
}

#[test]
fn empty_turns_yields_no_admits_and_no_writes() {
    let store = in_memory_store();
    let fx = FixtureExtractor::new();
    let turns: Vec<Turn> = Vec::new();
    let leaves = LeafIndex::new();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let opts = options(&*store, &ctx, &turns, &leaves, None);

    let out = ingest_raw_with_extractor(&opts, &fx).expect("ingest");
    assert!(out.admitted.is_empty(), "admitted: {:?}", out.admitted);
    assert!(!out.leaf_queryable);
    let jobs = store.pending_index_jobs(10, None, u32::MAX).expect("jobs");
    assert!(jobs.is_empty(), "index work: {jobs:?}");
}

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
