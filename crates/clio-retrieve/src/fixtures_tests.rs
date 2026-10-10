// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared test doubles and fixtures used by the
//! `clio-retrieve` test modules: an in-memory SQLite store, deterministic
//! embedders/rerankers/expanders, and a one-shot fake HTTP server.
//!
//! ## Owns
//! - Store/item/retriever fixtures and fake sidecar components
//! - `snapshot_of_bytes`, the one exact-byte-count snapshot fixture shared by
//!   the cap-boundary and hit-assembly tests
//!
//! ## Does not own
//! - Production gate/retrieval/compose behavior or its assertions
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::hybrid::HybridRetriever;
use crate::rerank::Reranker;
use crate::rerank_parse::RerankedDoc;
use crate::types::GraphExpander;
use clio_index::Embedder;
use clio_store::{IndexStore, SqliteStore, Store, VectorSlot};
use clio_types::{
    AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory,
    SourceType,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

/// In-memory SQLite store with a local dev KMS.
pub(crate) fn store() -> Arc<SqliteStore> {
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(clio_store::LocalDevKms::new());
    Arc::new(SqliteStore::open_in_memory(kms).expect("open in-memory sqlite"))
}

/// In-memory SQLite store plus its KMS handle (for shred/unreadable fixtures).
pub(crate) fn store_and_kms() -> (Arc<SqliteStore>, Arc<dyn clio_store::DekProvider>) {
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(clio_store::LocalDevKms::new());
    let store =
        Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("open in-memory sqlite"));
    (store, kms)
}

/// A [`FaultyKms`] that reports `code` for `subject_id` and delegates the rest.
struct FaultyKms {
    inner: clio_store::LocalDevKms,
    subject_id: String,
    code: ErrorCode,
}

impl clio_store::DekProvider for FaultyKms {
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError> {
        self.inner.ensure_dek(subject_id)
    }

    fn encrypt(
        &self,
        subject_id: &str,
        plaintext: &[u8],
    ) -> Result<clio_store::CiphertextEnvelope, AmError> {
        self.inner.encrypt(subject_id, plaintext)
    }

    fn decrypt(&self, envelope: &clio_store::CiphertextEnvelope) -> Result<Vec<u8>, AmError> {
        if envelope.subject_id == self.subject_id {
            return Err(AmError::new(self.code, "fixture fault"));
        }
        self.inner.decrypt(envelope)
    }

    fn destroy_dek(&self, subject_id: &str) -> Result<(), AmError> {
        self.inner.destroy_dek(subject_id)
    }

    fn diagnose_masked(&self, subject_id: &str) -> String {
        self.inner.diagnose_masked(subject_id)
    }

    fn blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.blind_index_key()
    }

    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.ensure_blind_index_key()
    }
}

/// Store whose rows for `subject_id` fail to open with `code`.
///
/// Lets a reader test assert how it reacts to a code without editing the
/// database. Writes still seal normally, so the row exists and is reachable; only
/// opening its content fails.
pub(crate) fn store_faulty_for(
    subject_id: &str,
    code: ErrorCode,
) -> (Arc<SqliteStore>, Arc<dyn clio_store::DekProvider>) {
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(FaultyKms {
        inner: clio_store::LocalDevKms::new(),
        subject_id: subject_id.to_owned(),
        code,
    });
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("open in-memory"));
    (store, kms)
}

/// Base episodic item with real ISO timestamps.
pub(crate) fn item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".into()),
        snapshot: Some(json!({"key": "value"})),
        gist: Some(gist.into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Semantic item in a chosen category.
pub(crate) fn semantic_item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    let mut base = item(id, bank, gist);
    base.kind = ItemKind::Semantic;
    base.category = Some(SemanticCategory::Schema);
    base.episodic_type = None;
    base
}

/// Belief-shaped episodic item with provenance and confidence.
pub(crate) fn belief_item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    let mut base = item(id, bank, gist);
    base.epistemic_kind = EpistemicKind::Belief;
    base.source_type = Some(SourceType::AgentInferred);
    base.confidence = Some(0.42);
    base
}

/// Persist an item and attach a lexical document.
pub(crate) fn put_lexical(store: &Arc<SqliteStore>, entry: &MemoryItem, text: &str) {
    store
        .create_memory_item(entry, "agent")
        .expect("create item");
    store
        .put_lexical_doc(&entry.bank_id, &entry.id, text)
        .expect("put lexical");
}

/// Persist an item and attach a dense vector.
pub(crate) fn put_vector(store: &Arc<SqliteStore>, entry: &MemoryItem, embedding: Vec<f32>) {
    store
        .create_memory_item(entry, "agent")
        .expect("create item");
    store
        .upsert_vector(&VectorSlot {
            item_id: entry.id.clone(),
            bank_id: entry.bank_id.clone(),
            model_id: clio_store::DEFAULT_EMBEDDING_MODEL.into(),
            embedding,
        })
        .expect("upsert vector");
}

/// Build a retriever over one store (no embedder/reranker by default).
pub(crate) fn retriever(store: &Arc<SqliteStore>) -> HybridRetriever {
    HybridRetriever::new(
        Arc::clone(store) as Arc<dyn Store>,
        Arc::clone(store) as Arc<dyn IndexStore>,
    )
}

/// Milliseconds in one day, for association decay fixtures.
pub(crate) const DAY_MILLIS: i64 = 86_400_000;

/// A snapshot whose compact serialization is exactly `target` bytes.
///
/// The filler is one ASCII byte per character, so the serialized length is tuned
/// rather than guessed and a change to the cap fails with the real length in the
/// message. A target at or below the empty skeleton's own length cannot be built
/// this way, so it is refused with both real lengths rather than padding by a
/// negative count.
pub(crate) fn snapshot_of_bytes(target: usize) -> Value {
    let skeleton = json!({ "unit_id": "u", "hub_id": "h", "filler": "" });
    let overhead = serde_json::to_string(&skeleton)
        .expect("serialize skeleton")
        .len();
    assert!(
        target > overhead,
        "target {target} is not above the empty skeleton overhead {overhead}"
    );
    let snapshot =
        json!({ "unit_id": "u", "hub_id": "h", "filler": "x".repeat(target - overhead) });
    let len = serde_json::to_string(&snapshot)
        .expect("serialize snapshot")
        .len();
    assert_eq!(len, target, "fixture is {len} bytes, wanted {target}");
    snapshot
}

/// Deterministic association engine over a fixture store (fixed clock).
pub(crate) fn assoc_engine_at(
    store: &Arc<SqliteStore>,
    now_millis: i64,
) -> crate::assoc::AssociationEngine {
    crate::assoc::AssociationEngine::with_clock(
        Arc::clone(store) as Arc<dyn Store>,
        clio_store::AssocPolicy::default(),
        now_millis,
    )
}

/// Deterministic 384-dim vector derived from a seed byte.
pub(crate) fn vector(seed: u8) -> Vec<f32> {
    (0..clio_store::EMBEDDING_DIMS)
        .map(|i| {
            let v = (u32::from(seed) + (i as u32 * 7)) % 97;
            v as f32 / 97.0
        })
        .collect()
}

/// Embedder that returns one fixed vector for every input.
pub(crate) struct FixedEmbedder {
    pub vector: Vec<f32>,
}

impl Embedder for FixedEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts.iter().map(|_| self.vector.clone()).collect())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        "fake/fixed".into()
    }

    fn dims(&self) -> usize {
        clio_store::EMBEDDING_DIMS
    }
}

/// Embedder whose calls always fail (sidecar outage simulation).
pub(crate) struct FailingEmbedder;

impl Embedder for FailingEmbedder {
    fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "embed endpoint connect failed (sidecar down?)",
        ))
    }

    fn health(&self) -> Result<(), AmError> {
        Err(AmError::new(ErrorCode::Internal, "sidecar down"))
    }

    fn model_id(&self) -> String {
        "fake/failing".into()
    }

    fn dims(&self) -> usize {
        clio_store::EMBEDDING_DIMS
    }
}

/// Embedder that returns no vectors (defensive empty-batch path).
pub(crate) struct EmptyEmbedder;

impl Embedder for EmptyEmbedder {
    fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(Vec::new())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        "fake/empty".into()
    }

    fn dims(&self) -> usize {
        clio_store::EMBEDDING_DIMS
    }
}

/// Reranker that reverses the shortlist. Each document keeps a distinct
/// normalized score (bound to its original index) so score-to-hit binding is
/// observable in tests.
pub(crate) struct ReverseReranker;

impl Reranker for ReverseReranker {
    fn rerank(
        &self,
        _query: &str,
        documents: &[String],
        _top_k: usize,
    ) -> Result<Vec<RerankedDoc>, AmError> {
        Ok((0..documents.len())
            .rev()
            .map(|i| RerankedDoc {
                index: i,
                score: Some(0.9 - i as f64 * 0.1),
            })
            .collect())
    }
}

/// Reranker that always fails (fail-open simulation).
pub(crate) struct FailReranker;

impl Reranker for FailReranker {
    fn rerank(
        &self,
        _query: &str,
        _documents: &[String],
        _top_k: usize,
    ) -> Result<Vec<RerankedDoc>, AmError> {
        Err(AmError::new(ErrorCode::Internal, "rerank sidecar down"))
    }
}

/// Graph expander returning a fixed id set.
pub(crate) struct StaticExpander {
    pub ids: Vec<String>,
}

impl GraphExpander for StaticExpander {
    fn expand(
        &self,
        _bank: &str,
        _seeds: &[String],
        _max_hops: usize,
    ) -> Result<Vec<String>, AmError> {
        Ok(self.ids.clone())
    }
}

/// Graph expander that always fails.
pub(crate) struct FailExpander;

impl GraphExpander for FailExpander {
    fn expand(
        &self,
        _bank: &str,
        _seeds: &[String],
        _max_hops: usize,
    ) -> Result<Vec<String>, AmError> {
        Err(AmError::new(ErrorCode::Internal, "graph unavailable"))
    }
}

/// One-shot fake HTTP server returning a canned raw response.
pub(crate) fn fake_http_server(response: String) -> (String, std::thread::JoinHandle<()>) {
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

/// One-shot fake HTTP server that also captures the raw request bytes so tests
/// can assert the wire shape (path, JSON body, auth header).
pub(crate) fn fake_http_capture_server(
    response: String,
) -> (
    String,
    std::sync::mpsc::Receiver<String>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Read to the Content-Length boundary: the client may deliver
            // headers and body in separate TCP segments.
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 16_384];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
                if let Some(len) = content_length(&buf) {
                    let head = buf
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .expect("headers");
                    if buf.len() >= head + 4 + len {
                        break;
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}"), rx, handle)
}

fn content_length(buf: &[u8]) -> Option<usize> {
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    head.lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
}

/// A guaranteed-dead localhost port (nothing listens).
pub(crate) fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}
