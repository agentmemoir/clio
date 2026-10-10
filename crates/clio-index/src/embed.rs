// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for embedding generation policy: the `Embedder`
//! contract, item-to-text assembly, best-effort secret redaction, and the
//! fail-closed dimension gate.
//!
//! ## Owns
//! - `Embedder` trait and `TeiEmbedder` sidecar adapter with bounded retries
//! - Embed/lexical text assembly (gist preferred, snapshot tokens included)
//! - Redaction of inline secrets before embed/index
//! - Dimension mismatch rejection (never silent truncation)
//!
//! ## Does not own
//! - HTTP transport mechanics (`http`)
//! - Vector storage or queue scheduling (`worker` / `clio_store`)
//!
//! ## Boundary
//!
//! Changes to embed text shape or retry policy should normally require changes here.
//! Changes to HTTP mechanics should not require changes here.

use crate::http::{get_ok, post_json};
use clio_types::{AmError, ErrorCode, MemoryItem};
use serde_json::{Value, json};
use std::time::Duration;

/// Default per-request sidecar timeout.
pub const DEFAULT_EMBED_TIMEOUT: Duration = Duration::from_secs(30);
/// Default bounded retries for transient sidecar failures.
pub const DEFAULT_EMBED_RETRIES: u32 = 2;
/// Base delay for exponential retry backoff (attempt n sleeps base × 2^n).
pub const RETRY_BASE_DELAY_MS: u64 = 200;

/// Dense-embedding provider (sidecar HTTP; never a host-process model load).
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts; the result must be 1:1 with the input order.
    ///
    /// # Errors
    /// Transport failures, non-2xx responses, or server-side model errors.
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError>;

    /// Sidecar health probe (2xx expected).
    ///
    /// # Errors
    /// Transport failures or non-2xx responses.
    fn health(&self) -> Result<(), AmError>;

    /// Embedding model id recorded on vector rows.
    fn model_id(&self) -> String;

    /// Configured vector dimension (fail-closed contract for responses).
    fn dims(&self) -> usize;
}

/// TEI-compatible sidecar client (`POST /embed`, `GET /health`).
///
/// `url` is the sidecar base URL (e.g. `http://127.0.0.1:34311`); `/embed` and
/// `/health` are appended. Responses must be a JSON array of `dims`-length
/// float arrays, 1:1 with the input batch — anything else fails closed.
#[derive(Debug)]
pub struct TeiEmbedder {
    url: String,
    model_id: String,
    dims: usize,
    bearer: Option<String>,
    timeout: Duration,
    retries: u32,
}

impl TeiEmbedder {
    /// Build a sidecar client from effective config values.
    ///
    /// `url` is the base URL; an empty URL fails closed (misconfiguration).
    ///
    /// # Errors
    /// Empty url/model, or zero dims.
    pub fn new(
        url: impl Into<String>,
        model_id: impl Into<String>,
        dims: usize,
        bearer: Option<String>,
    ) -> Result<Self, AmError> {
        let url = url.into();
        let model_id = model_id.into();
        if url.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.url is not configured; set embed.url or CLIO_EMBED_URL to the sidecar base URL",
            ));
        }
        if model_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.model must be a non-empty model id",
            ));
        }
        if dims == 0 {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.dims must be a positive dimension",
            ));
        }
        // Fail closed at construction for non-http/https endpoints and for a
        // remote plain-http one; the same parser and gate guard every request.
        crate::http::gate(&url)?;
        Ok(Self {
            url,
            model_id,
            dims,
            bearer,
            timeout: DEFAULT_EMBED_TIMEOUT,
            retries: DEFAULT_EMBED_RETRIES,
        })
    }

    /// Override the request timeout (tests).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    fn embed_once(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        let body = json!({ "inputs": texts });
        let url = format!("{}/embed", self.url.trim_end_matches('/'));
        let value = post_with_retry(
            &url,
            &body,
            self.timeout,
            self.bearer.as_deref(),
            self.retries,
        )?;
        parse_embeddings(value, texts.len(), self.dims, &self.model_id)
    }
}

/// POST one JSON body with the shared bounded retry policy (429/5xx/transport).
///
/// Both provider adapters share this so retry classification stays in one place.
pub(crate) fn post_with_retry(
    url: &str,
    body: &Value,
    timeout: Duration,
    bearer: Option<&str>,
    retries: u32,
) -> Result<Value, AmError> {
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        match post_json(url, body, timeout, bearer) {
            Ok(value) => return Ok(value),
            Err(e) if attempt <= retries && is_retryable(&e) => {
                let delay = RETRY_BASE_DELAY_MS.saturating_mul(2_u64.pow(attempt - 1));
                std::thread::sleep(Duration::from_millis(delay));
            }
            Err(e) => return Err(e),
        }
    }
}

impl Embedder for TeiEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        self.embed_once(texts)
    }

    fn health(&self) -> Result<(), AmError> {
        get_ok(
            &format!("{}/health", self.url.trim_end_matches('/')),
            self.timeout,
            self.bearer.as_deref(),
        )
    }

    fn model_id(&self) -> String {
        self.model_id.clone()
    }

    fn dims(&self) -> usize {
        self.dims
    }
}

/// Transient conditions worth one bounded retry: connect/IO errors and 429/5xx.
///
/// Shared with the `http` transport tests so error-text classification stays
/// in lockstep with the transport's failure mapping.
pub(crate) fn is_retryable(err: &AmError) -> bool {
    let text = &err.message;
    text.contains("connect failed")
        || text.contains("sidecar unreachable")
        || text.contains("read timed out")
        || text.contains("read deadline exceeded")
        || text.contains("HTTP 429")
        || text.contains("HTTP 5")
}

/// Parse a TEI `/embed` response, fail closed on shape/length mismatches.
fn parse_embeddings(
    value: Value,
    batch: usize,
    dims: usize,
    model_id: &str,
) -> Result<Vec<Vec<f32>>, AmError> {
    let vectors: Vec<Vec<f32>> = serde_json::from_value(value).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("embed response is not a float-vector array: {e}"),
        )
    })?;
    if vectors.len() != batch {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!(
                "embed response count {} does not match batch size {batch}",
                vectors.len()
            ),
        ));
    }
    for (i, v) in vectors.iter().enumerate() {
        if v.len() != dims {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "embed model {model_id} returned {} dims for input {i}; expected {dims} (dim mismatch fails closed; re-configure embed.dims and rebuild)",
                    v.len()
                ),
            ));
        }
    }
    Ok(vectors)
}

/// Embed/lexical input for one item: `None` when the item has no content halves.
///
/// Embed text prefers the prose gist (semantic recall); the lexical document
/// adds the snapshot's exact tokens (identifiers, versions, error codes).
/// Both go through best-effort inline secret scrubbing before leaving the
/// process — DEKs never reach the sidecar because only scrubbed text is sent.
#[must_use]
pub fn embed_text(item: &MemoryItem) -> Option<String> {
    let raw = if let Some(g) = item.gist.as_ref().filter(|g| !g.trim().is_empty()) {
        g.clone()
    } else {
        let snapshot = item.snapshot.as_ref()?;
        snapshot_text(snapshot)
    };
    Some(scrub_embed_text(&raw))
}

/// Full lexical document: gist plus snapshot tokens, scrubbed.
///
/// This is the plaintext the store receives on the lexical path; the store
/// replaces every stemmed token with a keyed digest before persisting, so the
/// returned string itself is never stored.
#[must_use]
pub fn lexical_text(item: &MemoryItem) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(gist) = item.gist.as_ref().filter(|g| !g.trim().is_empty()) {
        parts.push(gist.clone());
    }
    if let Some(snapshot) = &item.snapshot {
        parts.push(snapshot_text(snapshot));
    }
    if parts.is_empty() {
        return None;
    }
    Some(scrub_embed_text(&parts.join(" ")))
}

/// Compact JSON text of a snapshot (keys + values, identifier-heavy).
fn snapshot_text(snapshot: &serde_json::Value) -> String {
    serde_json::to_string(snapshot).expect("snapshot JSON serializes")
}

/// Best-effort inline secret redaction (full hygiene is owned by hygiene tooling).
#[must_use]
pub fn scrub_embed_text(text: &str) -> String {
    clio_config::secret::scrub_inline_secrets(text)
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    fn vector(v: &[f32]) -> Value {
        serde_json::to_value(v).expect("vector serializes")
    }

    #[test]
    fn parse_embeddings_rejects_bad_shapes() {
        // Non-array body.
        assert!(parse_embeddings(json!(5), 1, 384, "m").is_err());
        // Count mismatch.
        let err = parse_embeddings(json!([vector(&[0.1; 384])]), 2, 384, "m").unwrap_err();
        assert!(err.to_string().contains("does not match batch"), "{err}");
        // Dim mismatch.
        let err = parse_embeddings(json!([vector(&[0.1; 128])]), 1, 384, "m").unwrap_err();
        assert!(
            err.to_string().contains("dim mismatch fails closed"),
            "{err}"
        );
        // Correct shape passes through.
        let two = json!([vector(&[0.1; 384]), vector(&[0.2; 384])]);
        assert_eq!(parse_embeddings(two, 2, 384, "m").unwrap().len(), 2);
    }

    #[test]
    fn retryable_classification() {
        assert!(is_retryable(&AmError::new(
            ErrorCode::Internal,
            "embed endpoint connect failed (sidecar down?): boom"
        )));
        assert!(is_retryable(&AmError::new(
            ErrorCode::Internal,
            "x HTTP 429 y"
        )));
        assert!(is_retryable(&AmError::new(
            ErrorCode::Internal,
            "x HTTP 503 y"
        )));
        assert!(!is_retryable(&AmError::new(
            ErrorCode::Internal,
            "embed endpoint returned HTTP 404: nope"
        )));
        assert!(!is_retryable(&AmError::new(
            ErrorCode::OutOfRange,
            "dim mismatch"
        )));
    }

    #[test]
    fn empty_url_error_names_the_prefixed_env_variable() {
        // This message tells an operator which variable to set, so it must name
        // the variable the binary actually reads. The assertion is on the name
        // alone: the sentence around it is free to change, the name is not.
        let err = TeiEmbedder::new("", "m", 8, None).expect_err("empty url");
        assert_eq!(err.code, ErrorCode::ConfigCorrupt);
        assert!(err.message.contains("CLIO_EMBED_URL"), "{err}");
    }
}

#[cfg(test)]
#[path = "embed_tests.rs"]
mod embed_tests;
