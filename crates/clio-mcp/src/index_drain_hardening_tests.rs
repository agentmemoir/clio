// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the gated Postgres drain/retrieval path and
//! the durable backlog visibility checks that exercise the real index store.
//!
//! ## Owns
//! - A local TEI-compatible HTTP double and the Postgres live-path scenario
//! - The private schema that scopes this test's store-global drain report
//! - Retry/backlog setup and `maintenance_status` assertions
//! - The targeted one-item drain a one-shot write runs after the batch
//!
//! ## Does not own
//! - Index-drain production policy (`index_drain`)
//! - Retrieval fusion or store SQL
//!
//! ## Boundary
//!
//! Changes to Postgres drain or backlog regression coverage should normally require changes here.
//! Changes to production drain or retrieval behavior should not require changes here.
//!
//! Keep this module focused on index-drain regression coverage.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DENSE_DIMS: usize = 384;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn unique_id(prefix: &str) -> String {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{sequence}", std::process::id())
}

/// The shared `CLIO_DATABASE_URL`, but only when it points at a Postgres
/// backend. A missing or non-Postgres value fails closed unless the operator
/// opted out, which prints a note and makes the caller return early.
fn postgres_url() -> Option<String> {
    clio_store::pg_skip::pg_url_or_skip("the mcp Postgres drain-hardening test")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_args(bank: &str, id: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": id,
            "gist": "postgres durable drain retrieval",
            "snapshot": {"entity": "postgres durable drain retrieval"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": "postgres durable drain retrieval (1 note since 2024-01-01).",
        "bank": bank,
        "actor": "agent"
    })
}

fn hit_ids(value: &Value) -> Vec<String> {
    value["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .filter_map(|hit| hit["item_id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

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

/// A local TEI double that answers both the drain and retrieval embedding calls.
///
/// Hardened the same way as the double in `read_scores_tests` (issue #31): the
/// original read once per connection and answered `{}` to anything whose first
/// bytes were not `POST /embed`. Under parallel load TCP delivered a partial
/// request, the drain or the retrieve leg got a shape error instead of a
/// vector, and the dense arm degraded — which surfaced as a failure at the
/// `dense_candidates` assertion with no obvious cause. Reading until the
/// request line is complete removes that. The accept loop also tolerates a
/// transient `WouldBlock` and stops on a wall-clock deadline rather than on a
/// fixed request count, so a mis-served request cannot hang the test.
fn fake_tei_server() -> (String, JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake TEI server");
    let port = listener.local_addr().expect("fake TEI address").port();
    listener
        .set_nonblocking(true)
        .expect("nonblocking fake TEI listener");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut embed_requests = 0;
        while embed_requests < 2 && Instant::now() < deadline {
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
                        embed_requests += 1;
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
        embed_requests
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

fn backlog_item(bank: &str, id: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: id.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": "backlog item"})),
        gist: Some(format!("backlog item {id}")),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn sqlite_state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &json!({}))
        .expect("open sqlite state")
}

#[test]
fn postgres_store_drain_retrieve_covers_dense_and_lexical() {
    let Some(database_url) = postgres_url() else {
        return;
    };
    let _guard = clio_store::lock_pg_test();
    // The drain report is store-global, and the shared database carries queued
    // rows from other suites and earlier runs whose items are sealed under
    // process-local DEKs. Run this store in a private schema so the report
    // counts only the row this test created.
    let schema = clio_store::ScopedSchema::new(&database_url);
    let bank = unique_id("mcp-pg-drain-bank");
    let id = unique_id("mcp-pg-drain-item");
    let (tei_url, tei_server) = fake_tei_server();
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
            backend: "postgres".into(),
            database_url: schema.url().to_owned(),
            default_bank: bank.clone(),
            ..McpOpenOptions::default()
        },
        &effective,
    )
    .expect("open Postgres state");

    let stored = call(&state, "store", store_args(&bank, &id));
    assert_eq!(stored["ok"], json!(true), "{stored}");
    assert_eq!(stored["id"], json!(id));
    let jobs = state
        .index
        .pending_index_jobs(1_000_000, None, u32::MAX)
        .expect("read durable pending job");
    assert!(
        jobs.iter()
            .any(|job| job.bank_id == bank && job.item_id == id),
        "store must leave a durable outbox row: {jobs:?}"
    );
    let before = state.index_status_json(&bank).expect("status before drain");
    assert_eq!(before["pending_jobs"], json!(1));
    assert_eq!(before["dense_rows"], json!(0));
    assert_eq!(before["lexical_rows"], json!(0));

    let report = state.drain_index().expect("drain Postgres index");
    assert_eq!(report.applied, 1, "{report:?}");
    assert_eq!(report.failed, 0, "{report:?}");
    let retrieved = call(
        &state,
        "retrieve",
        json!({"query": "postgres durable drain retrieval", "bank": bank, "limit": 5}),
    );
    assert_eq!(retrieved["ok"], json!(true), "{retrieved}");
    assert!(hit_ids(&retrieved).contains(&id), "{retrieved}");
    assert_eq!(retrieved["dense_candidates"], json!(1), "{retrieved}");
    assert_eq!(retrieved["lexical_candidates"], json!(1), "{retrieved}");

    let after = state.index_status_json(&bank).expect("status after drain");
    assert_eq!(after["pending_jobs"], json!(0), "{after}");
    assert_eq!(after["dense_rows"], json!(1), "{after}");
    assert_eq!(after["lexical_rows"], json!(1), "{after}");
    assert!(
        state
            .store
            .delete_item(&bank, &id)
            .expect("remove test item")
    );
    drop(state);
    assert_eq!(tei_server.join().expect("join fake TEI server"), 2);
}

#[test]
fn maintenance_status_keeps_failed_backlog_visible_after_wait() {
    let state = sqlite_state();
    let bank = unique_id("mcp-backlog-bank");
    let count = state.index_drain.batch_size() + 1;
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let id = unique_id("mcp-backlog-item");
        state
            .store
            .create_memory_item(&backlog_item(&bank, &id), "agent")
            .expect("seed backlog item");
        ids.push(id);
    }
    let queued = state
        .index
        .pending_index_jobs(count + 1, None, u32::MAX)
        .expect("read seeded outbox");
    assert_eq!(
        queued.iter().filter(|job| job.bank_id == bank).count(),
        count
    );

    let error = "synthetic backlog embed failure";
    for id in &ids {
        state
            .index
            .fail_index_job(&bank, id, error)
            .expect("record retry state");
    }
    let sweeper = state.spawn_index_sweeper_with(Duration::from_millis(10));
    state.nudge_index();
    // Every seeded job failed inside the 30s retry backoff, so a sweeper drain
    // skips them and cannot change the status below. The fixed 2.1s sleep this
    // replaced was shorter than that backoff, so it could not make them due.
    let status = call(&state, "maintenance_status", json!({"bank": bank}));
    assert_eq!(status["ok"], json!(true), "{status}");
    let index = &status["index"];
    assert_eq!(index["pending_jobs"], json!(count), "{status}");
    assert_eq!(index["retry_jobs"], json!(count), "{status}");
    assert_eq!(index["coverage_last_error"], json!(error), "{status}");
    drop(state);
    sweeper.join().expect("join backlog sweeper");
}

/// A one-shot write's targeted drain indexes the one item it stored, so the item
/// is searchable without waiting on the bounded batch; a second drain for the
/// same item finds nothing to do.
#[test]
fn a_targeted_drain_indexes_the_item_a_write_stored() {
    let state = sqlite_state();
    let bank = unique_id("mcp-item-drain-bank");
    let id = unique_id("mcp-item-drain-item");
    let stored = call(&state, "store", store_args(&bank, &id));
    assert_eq!(stored["ok"], json!(true), "{stored}");
    assert_eq!(stored["id"], json!(id));

    let report = state
        .drain_index_item(&bank, &id)
        .expect("drain the stored item");
    assert_eq!(report.lexical_only, 1, "{report:?}");
    assert_eq!(report.failed, 0, "{report:?}");

    let retrieved = call(
        &state,
        "retrieve",
        json!({"query": "postgres durable drain retrieval", "bank": bank, "limit": 5}),
    );
    assert!(hit_ids(&retrieved).contains(&id), "{retrieved}");

    // The job is gone, so a second targeted drain finds nothing to do.
    let again = state
        .drain_index_item(&bank, &id)
        .expect("drain the stored item again");
    assert_eq!(
        again.applied + again.lexical_only + again.skipped + again.failed,
        0,
        "{again:?}"
    );
}
