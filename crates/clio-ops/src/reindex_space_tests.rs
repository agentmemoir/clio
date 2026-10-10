// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the end-to-end `reindex` suite across two
//! embedding spaces and two providers: a mock TEI endpoint at one width and a
//! mock OpenAI-compatible endpoint at another.
//!
//! ## Owns
//! - The in-process mock embed server (`TEI` and `OpenAI` response shapes)
//! - `reindex` → dense rows → `KNN` assertions at 768 and 1536 dims
//!
//! ## Does not own
//! - Reindex policy itself (`reindex`) or the rebuild sweep (`clio_index::rebuild`)
//! - Provider dialects (`clio_index` adapters)
//!
//! ## Boundary
//!
//! Changes to the ops dense-reindex path across embedding spaces should normally
//! update the suite here. Changes to reindex policy should not.
//!
//! Keep this module focused on the cross-provider reindex end-to-end path.

use crate::reindex::{ReindexArgs, reindex};
use clio_index::RebuildTargets;
use clio_store::{EmbeddingSpace, IndexStore, SqliteStore, Store};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const TS: &str = "2026-01-01T00:00:00Z";

fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(clio_store::LocalDevKms::new())).expect("in-memory store")
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".to_owned()),
        snapshot: Some(json!({"error_code": "ECONNRESET"})),
        gist: Some("space switch fixture".to_owned()),
        valid_from: TS.to_owned(),
        valid_until: None,
        tx_from: TS.to_owned(),
        tx_until: None,
        created_at: TS.to_owned(),
        updated_at: TS.to_owned(),
        context: None,
    }
}

/// Minimal in-process embed server answering both provider dialects at `dims`.
struct MockEmbed {
    url: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockEmbed {
    fn spawn(dims: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock bind");
        let port = listener.local_addr().expect("local addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            listener.set_nonblocking(true).expect("nonblocking");
            loop {
                if stop_thread.load(Ordering::SeqCst) {
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).expect("blocking");
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let request = read_http_request(&mut stream);
                        let body = mock_body(&request, dims);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for MockEmbed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Read one complete HTTP request (headers plus declared body) from `stream`.
///
/// A single `read` can return only the request head while the body is still in
/// flight, which made the mock mis-count a two-item batch as one. Loop until the
/// header terminator and the declared `Content-Length` body are both received,
/// or the peer closes or the read times out.
fn read_http_request(stream: &mut TcpStream) -> String {
    let mut received = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            break;
        }
        received.extend_from_slice(&chunk[..read]);
        let Some(headers_end) = received.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&received[..headers_end]);
        let body_end = headers_end + 4 + content_length(&head);
        if received.len() >= body_end {
            break;
        }
    }
    String::from_utf8_lossy(&received).to_string()
}

/// Declared `Content-Length` in an HTTP request head, or 0 when absent.
fn content_length(head: &str) -> usize {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

/// Response body for the dialect named by the request line and path.
fn mock_body(request: &str, dims: usize) -> String {
    let count = request_input_count(request);
    let vector = vec![0.5_f32; dims];
    if request.starts_with("POST /v1/embeddings") {
        let data: Vec<Value> = (0..count)
            .map(|i| json!({"object": "embedding", "index": i, "embedding": vector}))
            .collect();
        json!({"object": "list", "data": data}).to_string()
    } else if request.starts_with("POST /embed") {
        let rows: Vec<Value> = (0..count).map(|_| json!(vector)).collect();
        Value::Array(rows).to_string()
    } else {
        "{}".to_owned()
    }
}

/// Input count from a `TEI` (`inputs`) or `OpenAI` (`input`) request body.
fn request_input_count(request: &str) -> usize {
    let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    value
        .get("inputs")
        .or_else(|| value.get("input"))
        .and_then(Value::as_array)
        .map_or(1, Vec::len)
}

/// Switch `store` to `space`, seed two items, and reindex dense through `provider`.
fn reindex_in_space(store: &SqliteStore, provider: &str, space: &EmbeddingSpace) {
    let bank = format!("b-{}", space.dims);
    let id1 = format!("i1-{}", space.dims);
    let id2 = format!("i2-{}", space.dims);
    store
        .create_memory_item(&item(&id1, &bank), "actor")
        .unwrap();
    store
        .create_memory_item(&item(&id2, &bank), "actor")
        .unwrap();
    let server = MockEmbed::spawn(space.dims());
    let embedder = clio_index::build_embedder(
        provider,
        server.url.clone(),
        space.model_id(),
        space.dims(),
        None,
    )
    .expect("provider builds");
    let report = reindex(&ReindexArgs {
        store,
        index: store,
        embedder: Some(&*embedder),
        bank: Some(&bank),
        target: RebuildTargets::Dense,
        dry_run: false,
        confirm: true,
    })
    .expect("reindex succeeds");
    assert_eq!(report.dense_applied, 2, "{provider} @ {}", space.dims());
    let coverage = store.index_coverage(Some(&bank)).unwrap();
    assert_eq!(coverage.dense_rows, 2);
    let hits = store.knn(&bank, &vec![0.5_f32; space.dims()], 2).unwrap();
    assert_eq!(hits.len(), 2, "{provider} @ {} knn", space.dims());
}

#[test]
fn reindex_across_two_providers_and_widths() {
    let mut store = store();

    // TEI at 768 dims.
    let tei_space = EmbeddingSpace::new("local/nomic-embed-text", 768).unwrap();
    let report = store.apply_configured_space(&tei_space, true).unwrap();
    assert!(report.switched);
    reindex_in_space(&store, "tei", &tei_space);
    assert_eq!(store.embedding_space().unwrap(), tei_space);

    // OpenAI-compatible at 1536 dims: another confirmed switch, another rebuild.
    let openai_space = EmbeddingSpace::new("text-embedding-3-small", 1536).unwrap();
    let report = store.apply_configured_space(&openai_space, true).unwrap();
    assert!(report.switched);
    assert_eq!(report.purged_rows, 2, "the 768-space rows are discarded");
    reindex_in_space(&store, "openai", &openai_space);
    assert_eq!(store.embedding_space().unwrap(), openai_space);
}
