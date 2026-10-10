// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for test doubles used by the embed and worker tests.
//!
//! ## Owns
//! - Deterministic embedder fakes (`FakeEmbedder`, `FailEmbedder`, dim/swap variants)
//! - The local fake TEI HTTP server and shared worker/rebuild fixtures
//!
//! ## Does not own
//! - Production embed transport or text assembly (`embed`/`http`)
//! - The assertions themselves (owned by the `*_tests` modules)
//!
//! ## Boundary
//!
//! Changes to test-double behavior should normally require changes here.
//! Changes to production embeddings should not require changes here.
//!
//! Keep this module focused on test doubles and fixtures.

use crate::embed::Embedder;
use clio_store::{DEFAULT_EMBEDDING_MODEL, EMBEDDING_DIMS};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// Deterministic 384-dim vector for a text: stable across runs.
pub fn fake_vector(text: &str, dims: usize) -> Vec<f32> {
    let seed: u64 = text.bytes().map(|b| u64::from(b) % 97).sum::<u64>() + 1;
    (0..dims)
        .map(|i| {
            // Values stay in [0, 97); the exact mantissa is irrelevant.
            #[allow(clippy::cast_precision_loss)]
            let v = ((seed * (i as u64 + 7)) % 97) as f32 / 97.0;
            v
        })
        .collect()
}

/// In-process deterministic embedder (no HTTP).
pub struct FakeEmbedder {
    model: String,
    dims: usize,
}

impl FakeEmbedder {
    /// Schema-compatible fake (384 dims).
    pub fn new() -> Self {
        Self {
            model: DEFAULT_EMBEDDING_MODEL.into(),
            dims: EMBEDDING_DIMS,
        }
    }
}

impl Embedder for FakeEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts.iter().map(|t| fake_vector(t, self.dims)).collect())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        self.model.clone()
    }

    fn dims(&self) -> usize {
        self.dims
    }
}

/// Embedder whose calls always fail (sidecar outage simulation).
pub struct FailEmbedder;

impl Embedder for FailEmbedder {
    fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "embed endpoint connect failed (sidecar down?): simulated outage",
        ))
    }

    fn health(&self) -> Result<(), AmError> {
        Err(AmError::new(ErrorCode::Internal, "sidecar down"))
    }

    fn model_id(&self) -> String {
        "fake/failing".to_owned()
    }

    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

/// Wrong-dimension embedder (dim-mismatch simulation: claims the schema
/// dimension at construction, returns 128-dim vectors).
pub struct WrongDimsEmbedder;

impl Embedder for WrongDimsEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts.iter().map(|t| fake_vector(t, 128)).collect())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        "fake/wrong-dims".to_owned()
    }

    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

/// Runtime-swappable embedder (restore-sidecar scenarios).
pub struct SwapEmbedder {
    current: Mutex<Arc<dyn Embedder>>,
}

impl SwapEmbedder {
    /// Start in the given mode.
    pub fn new(initial: Arc<dyn Embedder>) -> Self {
        Self {
            current: Mutex::new(initial),
        }
    }

    /// Switch modes.
    pub fn swap(&self, next: Arc<dyn Embedder>) {
        *self.current.lock().expect("swap embedder") = next;
    }
}

impl Embedder for SwapEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        self.current
            .lock()
            .expect("swap embedder")
            .embed_batch(texts)
    }

    fn health(&self) -> Result<(), AmError> {
        self.current.lock().expect("swap embedder").health()
    }

    fn model_id(&self) -> String {
        self.current.lock().expect("swap embedder").model_id()
    }

    fn dims(&self) -> usize {
        self.current.lock().expect("swap embedder").dims()
    }
}

/// Local fake TEI server: deterministic vectors, bounded lifetime.
pub struct FakeEmbedServer {
    /// Base URL (`http://127.0.0.1:<port>`).
    pub url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    requests: Arc<AtomicUsize>,
    last_auth: Arc<Mutex<Option<String>>>,
}

impl FakeEmbedServer {
    /// Spawn the accept loop on an ephemeral port (200 responses).
    pub fn spawn() -> Self {
        Self::spawn_with(Duration::ZERO, 200, None)
    }

    /// Spawn a server that delays each response.
    pub fn spawn_slow(delay: Duration) -> Self {
        Self::spawn_with(delay, 200, None)
    }

    /// Spawn a server answering with the given HTTP status.
    pub fn spawn_status(status: u16) -> Self {
        Self::spawn_with(Duration::ZERO, status, None)
    }

    /// Spawn a server whose every response is these exact raw bytes.
    pub fn spawn_raw(response: Vec<u8>) -> Self {
        Self::spawn_with(Duration::ZERO, 200, Some(response))
    }

    fn spawn_with(delay: Duration, status: u16, raw: Option<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake bind");
        let port = listener.local_addr().expect("local addr").port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let last_auth = Arc::new(Mutex::new(None));
        let requests_thread = Arc::clone(&requests);
        let last_auth_thread = Arc::clone(&last_auth);
        let shutdown_thread = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            listener.set_nonblocking(true).expect("nonblocking");
            loop {
                if shutdown_thread.load(Ordering::SeqCst) {
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).expect("blocking");
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        if let Some((_, headers, body)) = read_request(&mut stream) {
                            if let Some(auth) = header(&headers, "authorization") {
                                *last_auth_thread.lock().expect("auth log") = Some(auth);
                            }
                            requests_thread.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(delay);
                            let resp = if let Some(raw) = &raw {
                                String::from_utf8_lossy(raw).to_string()
                            } else if status == 200 {
                                let json = fake_response(&body, EMBEDDING_DIMS);
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                                    json.len()
                                )
                            } else {
                                format!(
                                    "HTTP/1.1 {status} Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                                )
                            };
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.flush();
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            shutdown,
            handle: Some(handle),
            requests,
            last_auth,
        }
    }

    /// Total requests served.
    pub fn served(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// Last Authorization header seen.
    pub fn last_auth(&self) -> Option<String> {
        self.last_auth.lock().expect("auth log").clone()
    }
}

impl Drop for FakeEmbedServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, Vec<String>, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_len = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some(i) = text.find("\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 1 << 20 {
            return None;
        }
    };
    let head_text = String::from_utf8_lossy(&buf[..head_len]).to_string();
    let mut lines = head_text.lines();
    let request_line = lines.next().unwrap_or("").to_string();
    let headers: Vec<String> = lines.map(str::to_owned).collect();
    let content_length: usize = header(&headers, "content-length")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < head_len + content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return None,
        }
    }
    let body = String::from_utf8_lossy(&buf[head_len..head_len + content_length]).to_string();
    Some((request_line, headers, body))
}

fn header(headers: &[String], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|h| h.to_ascii_lowercase().starts_with(name))
        .and_then(|h| h.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

/// Build a TEI-shaped JSON response for the fake server body.
fn fake_response(body: &str, dims: usize) -> String {
    let parsed: Value = serde_json::from_str(body).unwrap_or(json!({}));
    let texts: Vec<String> = match parsed.get("inputs") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    let vectors: Vec<Vec<f32>> = texts.iter().map(|t| fake_vector(t, dims)).collect();
    json!(vectors).to_string()
}

/// A guaranteed-dead localhost port (nothing listens).
pub fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

// ---- worker/rebuild test fixtures (SQLite store + deterministic embedders) ----

use crate::worker::IndexCoordinator;
use clio_store::{IndexStore, SqliteStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem};

pub(crate) fn store() -> Arc<SqliteStore> {
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(clio_store::LocalDevKms::new());
    Arc::new(SqliteStore::open_in_memory(kms).unwrap())
}

/// Restore one legacy-format item row (body not DualContent v1) through the
/// opaque `ciphertext_backup` route, which is how such a row arrives from an
/// exporting node. The worker's skip path needs such a row to skip.
pub(crate) fn restore_legacy_format_row(store: &SqliteStore, bank: &str, id: &str) {
    use clio_store::{BatchStore, BatchWrite, OpaqueItemRecord};
    store
        .apply_batch(&[BatchWrite::PutOpaqueItem {
            item: OpaqueItemRecord {
                id: id.into(),
                bank_id: bank.into(),
                subject_id: format!("subj-{bank}"),
                kind: "episodic".into(),
                category: None,
                episodic_type: Some("temporal".into()),
                epistemic_kind: "fact".into(),
                source_type: None,
                confidence: None,
                admission_score: None,
                source_ref: None,
                valid_from: "2026-01-01T00:00:00Z".into(),
                valid_until: None,
                tx_from: "2026-01-01T00:00:00Z".into(),
                tx_until: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                content_cipher: "legacy-opaque-cipher".into(),
            },
            actor: "actor".into(),
        }])
        .unwrap();
}

pub(crate) fn leaf(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".into()),
        snapshot: Some(serde_json::json!({"error_code": "ECONNRESET"})),
        gist: Some(gist.into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

pub(crate) fn coordinator(
    store: &Arc<SqliteStore>,
    embedder: Arc<dyn crate::embed::Embedder>,
) -> IndexCoordinator {
    IndexCoordinator::new(
        Arc::clone(store) as Arc<dyn Store>,
        Arc::clone(store) as Arc<dyn IndexStore>,
        embedder,
        10,
    )
    .unwrap()
}
