// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the optional second-stage rerank clients
//! (TEI sidecar and Cohere-compatible hosted API) and provider selection from
//! the effective config.
//!
//! ## Owns
//! - `Reranker` trait and the HTTP adapters (`HttpReranker` TEI sidecar
//!   `POST /rerank` with `{query, texts, raw_scores}`, `CohereReranker` hosted
//!   `POST /v1/rerank`)
//! - The reranker factory (`build_reranker`/`build_reranker_from`) resolving
//!   provider/url/model/bearer from effective config, fail-closed
//!
//! ## Does not own
//! - Response parsing, the `RerankedDoc` pair shape, or the per-provider
//!   score normalization (`rerank_parse`, which owns that spec)
//! - First-stage dense/lexical retrieval or fusion (`hybrid`/`fusion`)
//! - Fail-open policy (the orchestrator keeps the fused order on any error)
//! - Attaching the built reranker to the live retriever (runtime owner)
//! - Writing the normalized value onto hits (`hybrid_rank`/`finalize`)
//!
//! ## Boundary
//!
//! Changes to the rerank sidecar contract should normally require changes here.
//! Changes to fusion or candidate generation should not require changes here.
//!
//! # Failure policy
//!
//! Rerank is precision-only: it reorders a bounded shortlist and can never
//! recover a document the first stage missed. Any error (timeout, transport,
//! malformed body) is surfaced to the orchestrator, which keeps the fused
//! order (fail-open) and records a structured warning. Rerank never blocks
//! compose indefinitely because the HTTP client has a bounded deadline.
//!
//! # Score contract
//!
//! The trait returns ordered `RerankedDoc` pairs carrying the provider's
//! normalized relevance in `[0, 1]`. The parse rules, the omitted-document
//! score rule (`None`, never a fabricated value), and the per-provider
//! normalization spec are documented and owned by `rerank_parse`.

use crate::rerank_parse::{RerankedDoc, parse_rerank_response};
use clio_index::{HttpEndpoint, post_json};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Default rerank request deadline (a small fraction of the retrieve SLO).
pub const DEFAULT_RERANK_TIMEOUT: Duration = Duration::from_secs(2);

/// Default number of documents the TEI sidecar accepts in one rerank request.
///
/// The sidecar enforces its own client batch ceiling and rejects a larger
/// request outright, so a shortlist longer than this is split into sequential
/// requests of at most this many documents and merged back into one
/// index-consistent result.
pub const DEFAULT_RERANK_MAX_BATCH: usize = 32;

/// The provider an unset `rerank.provider` resolves to: the tei sidecar.
///
/// The effective document carries the empty system default for an omitted
/// value, and config validation refuses to store an explicitly empty provider,
/// so the empty read here is the unset representation, not a stored value.
const DEFAULT_RERANK_PROVIDER: &str = "tei";

/// Reorders a bounded shortlist of documents best-first, carrying each
/// provider score alongside its index.
pub trait Reranker: Send + Sync {
    /// Return every document index in `0..documents.len()` (best-first), each
    /// with its normalized relevance; documents the provider omitted are
    /// appended in original order with `score: None`.
    ///
    /// # Errors
    /// Transport, timeout, or malformed-response failures.
    fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<RerankedDoc>, AmError>;
}

/// HTTP sidecar reranker (`POST {url}/rerank`).
pub struct HttpReranker {
    url: String,
    bearer: Option<String>,
    timeout: Duration,
    max_batch: usize,
}

impl fmt::Debug for HttpReranker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The bearer never appears in debug output; it is a secret.
        f.debug_struct("HttpReranker")
            .field("url", &self.url)
            .field("bearer", &self.bearer.as_ref().map(|_| "****"))
            .field("timeout", &self.timeout)
            .field("max_batch", &self.max_batch)
            .finish()
    }
}

impl HttpReranker {
    /// Build a client; an empty URL fails closed (misconfiguration).
    ///
    /// # Errors
    /// Empty or non-`http://` URL.
    pub fn new(url: impl Into<String>, bearer: Option<String>) -> Result<Self, AmError> {
        let url = url.into();
        if url.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "rerank.url is not configured; pass the sidecar base URL",
            ));
        }
        HttpEndpoint::parse(&url)?;
        Ok(Self {
            url,
            bearer,
            timeout: DEFAULT_RERANK_TIMEOUT,
            max_batch: DEFAULT_RERANK_MAX_BATCH,
        })
    }

    /// Override the request deadline (tests / tuning).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Override the documents-per-request ceiling (tests / tuning).
    ///
    /// A request must carry at least one document, so `0` is clamped to `1`.
    pub fn set_max_batch(&mut self, max_batch: usize) {
        self.max_batch = max_batch.max(1);
    }
}

impl Reranker for HttpReranker {
    fn rerank(
        &self,
        query: &str,
        documents: &[String],
        _top_k: usize,
    ) -> Result<Vec<RerankedDoc>, AmError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/rerank", self.url.trim_end_matches('/'));
        let batch = self.max_batch.max(1);
        let mut merged: Vec<RerankedDoc> = Vec::with_capacity(documents.len());
        let mut start = 0;
        for chunk in documents.chunks(batch) {
            // TEI `RerankRequest` = `{query, texts, raw_scores}`; `top_k` is not
            // a TEI field, so it is intentionally omitted from the wire body.
            // `raw_scores: false` is pinned so the sidecar returns its sigmoid
            // score in `[0, 1]` even if the server default ever drifts.
            let body = json!({
                "query": query,
                "texts": chunk,
                "raw_scores": false,
            });
            let value = post_json(&url, &body, self.timeout, self.bearer.as_deref())?;
            // Each response indexes its own chunk, so a chunk-local index is
            // shifted by the chunk's offset in `documents` before merging.
            for mut doc in parse_rerank_response(&value, chunk.len())? {
                doc.index += start;
                if doc.index >= documents.len() {
                    return Err(AmError::new(
                        ErrorCode::Internal,
                        "rerank index out of range after batch merge",
                    ));
                }
                merged.push(doc);
            }
            start += chunk.len();
        }
        Ok(merged)
    }
}

/// Cohere-compatible hosted reranker (`POST {url}/v1/rerank`).
pub struct CohereReranker {
    url: String,
    model: String,
    bearer: Option<String>,
    timeout: Duration,
}

impl fmt::Debug for CohereReranker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The bearer never appears in debug output; it is a secret.
        f.debug_struct("CohereReranker")
            .field("url", &self.url)
            .field("model", &self.model)
            .field("bearer", &self.bearer.as_ref().map(|_| "****"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl CohereReranker {
    /// Build a client; an empty URL or model fails closed (misconfiguration).
    ///
    /// # Errors
    /// Empty or non-`http(s)://` URL, or an empty model id.
    pub fn new(
        url: impl Into<String>,
        model: impl Into<String>,
        bearer: Option<String>,
    ) -> Result<Self, AmError> {
        let url = url.into();
        let model = model.into();
        if url.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "rerank.url is not configured; pass the provider base URL",
            ));
        }
        if model.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "rerank.model is required for rerank.provider = cohere",
            ));
        }
        HttpEndpoint::parse(&url)?;
        Ok(Self {
            url,
            model,
            bearer,
            timeout: DEFAULT_RERANK_TIMEOUT,
        })
    }

    /// Override the request deadline (tests / tuning).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }
}

impl Reranker for CohereReranker {
    fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<RerankedDoc>, AmError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({
            "query": query,
            "documents": documents,
            "top_n": top_k.min(documents.len()),
            "model": self.model,
        });
        // Tolerate operators passing a base URL with a `/v1` suffix (OpenAI-style
        // convention); the wire contract appends `/v1/rerank` exactly once.
        let base = self.url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let url = format!("{base}/v1/rerank");
        let value = post_json(&url, &body, self.timeout, self.bearer.as_deref())?;
        parse_rerank_response(&value, documents.len())
    }
}

/// Build the reranker selected by the effective config (`rerank.provider`).
///
/// Reads the process environment only through the config layer. `Ok(None)`
/// means "no reranker configured" (`rerank.url` empty), matching the
/// pre-adapter behavior.
///
/// # Errors
/// Unknown `rerank.provider`, a missing `rerank.model`/bearer for the selected
/// provider, or an endpoint the adapter rejects.
pub fn build_reranker() -> Result<Option<Arc<dyn Reranker>>, AmError> {
    let runtime = clio_config::Runtime::new();
    build_reranker_from(&runtime.effective_raw(), runtime.bearer_for("rerank"))
}

/// Factory over resolved config values (no environment reads here).
///
/// Fail-closed rules: unknown provider values are rejected; an empty
/// `rerank.url` disables rerank (`Ok(None)`); an empty `rerank.model` fails
/// closed once `rerank.url` is set; an unset provider — absent, null, or the
/// empty system default — resolves to the tei sidecar
/// (`DEFAULT_RERANK_PROVIDER`), while config validation refuses to store an
/// explicitly empty provider; `rerank.provider = "cohere"` requires a bearer
/// key (hosted provider) and `rerank.model`.
///
/// # Errors
/// As above; messages never contain key material.
pub fn build_reranker_from(
    effective: &Value,
    bearer: Option<String>,
) -> Result<Option<Arc<dyn Reranker>>, AmError> {
    let text = |pointer: &str| {
        effective
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let url = text("/rerank/url");
    if url.is_empty() {
        return Ok(None);
    }
    let provider = match effective.pointer("/rerank/provider") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_owned(),
        _ => DEFAULT_RERANK_PROVIDER.to_owned(),
    };
    let model = text("/rerank/model");
    match provider.as_str() {
        "tei" => {
            if model.is_empty() {
                return Err(AmError::new(
                    ErrorCode::ConfigCorrupt,
                    "rerank.model is required when rerank.url is set",
                ));
            }
            let mut client = HttpReranker::new(url, bearer)?;
            // An absent key keeps `DEFAULT_RERANK_MAX_BATCH`; a key of `0` is
            // not a cap the sidecar could accept, so the setter clamps it.
            if let Some(max_batch) = effective
                .pointer("/rerank/max_batch")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
            {
                client.set_max_batch(max_batch);
            }
            Ok(Some(Arc::new(client) as Arc<dyn Reranker>))
        }
        "cohere" => {
            let Some(key) = bearer else {
                return Err(AmError::new(
                    ErrorCode::ConfigCorrupt,
                    "rerank.provider = cohere requires credentials.rerank_api_key (or credentials.api_key)",
                ));
            };
            CohereReranker::new(url, model, Some(key))
                .map(|r| Some(Arc::new(r) as Arc<dyn Reranker>))
        }
        other => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!("rerank.provider must be tei|cohere, got `{other}`"),
        )),
    }
}

#[cfg(test)]
#[path = "rerank_tests.rs"]
pub(crate) mod rerank_tests;

#[cfg(test)]
#[path = "rerank_score_tests.rs"]
mod rerank_score_tests;

#[cfg(test)]
#[path = "rerank_batch_tests.rs"]
mod rerank_batch_tests;
