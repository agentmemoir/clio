// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for pluggable extraction backends, candidates,
//! and the extraction HTTP transport.
//!
//! ## Owns
//! - `Extractor` trait, fixture/poison adapters, template API adapter
//! - `Transport` trait (JSON post + optional bearer) and the live
//!   `HttpTransport` riding on the shared HTTPS client
//!
//! ## Does not own
//! - Span verification or gated admission
//! - Chat-completions request/prompt shape (`extract_chat`)
//! - Provider selection from config (`extract_factory`)
//!
//! ## Boundary
//!
//! Changes to extract I/O shape should normally require changes here.
//! Changes to verify rules should not require changes here.

use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;

/// Input to one extraction call.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractRequest {
    /// Source turn / tool payload text (also used later as span haystack).
    pub source_text: String,
    /// Optional structured tool payload alongside prose.
    pub tool_payload: Option<Value>,
    /// JSON template describing the desired snapshot shape.
    pub schema_template: Value,
    /// Optional verifier failure feedback for the single retry.
    pub verify_feedback: Option<String>,
    /// Optional untrusted descriptive context about the setting (bounded by
    /// `clio_types::validate_context` at ingest). Descriptive only: never a
    /// snapshot source and never followed as instructions.
    pub context: Option<String>,
}

/// Candidate output — not an admitted item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractCandidate {
    /// Schema-typed snapshot when structural extract succeeded.
    pub snapshot: Option<Value>,
    /// Optional non-authoritative prose gist.
    pub gist: Option<String>,
    /// Provenance id (maps to `MemoryItem.source_ref`).
    pub source_ref: String,
    /// Free-form diagnostic blob for offline review.
    pub diagnostic: Option<Value>,
}

/// Pluggable extractor (API Profile A/B, local NuExtract Profile C, or fixtures).
///
/// `Send + Sync` is required because parallel chunk workers share one extractor
/// across threads; every adapter (template API, hosted chat, fixtures) is
/// stateless or internally synchronized.
pub trait Extractor: std::fmt::Debug + Send + Sync {
    /// Produce one candidate snapshot from source text + template.
    ///
    /// # Errors
    /// Transport / parse failures.
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError>;
}

/// Recorded fixture map: exact `source_text` → candidate (CI-safe).
#[derive(Debug, Default)]
pub struct FixtureExtractor {
    by_source: HashMap<String, ExtractCandidate>,
}

impl FixtureExtractor {
    /// Empty fixture table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one golden source → candidate pair.
    pub fn insert(&mut self, source_text: impl Into<String>, candidate: ExtractCandidate) {
        self.by_source.insert(source_text.into(), candidate);
    }
}

impl Extractor for FixtureExtractor {
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
        self.by_source
            .get(&req.source_text)
            .cloned()
            .ok_or_else(|| {
                AmError::new(
                    ErrorCode::InvalidArgument,
                    "fixture extractor has no entry for this source_text",
                )
            })
    }
}

/// Extractor that always returns an ungrounded paraphrase snapshot (for retry tests).
#[derive(Debug)]
pub struct PoisonExtractor {
    /// Provenance id stamped on candidates.
    pub source_ref: String,
    /// How many times `extract` has been called.
    calls: Mutex<u32>,
}

impl PoisonExtractor {
    /// New poison extractor.
    #[must_use]
    pub fn new(source_ref: impl Into<String>) -> Self {
        Self {
            source_ref: source_ref.into(),
            calls: Mutex::new(0),
        }
    }

    /// Number of extract invocations so far.
    #[must_use]
    pub fn call_count(&self) -> u32 {
        *self.calls.lock().expect("poison extractor lock")
    }
}

impl Extractor for PoisonExtractor {
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
        let mut n = self.calls.lock().expect("poison extractor lock");
        *n += 1;
        Ok(ExtractCandidate {
            snapshot: Some(json!({"entity": "NOT_IN_SOURCE_PARAPHRASE"})),
            gist: Some(format!("gist from call {}", *n)),
            source_ref: self.source_ref.clone(),
            diagnostic: Some(json!({
                "poison": true,
                "feedback": req.verify_feedback,
            })),
        })
    }
}

/// HTTP-ish transport used by [`TemplateApiExtractor`] (injectable; no live I/O in CI).
pub trait Transport: std::fmt::Debug + Send + Sync {
    /// POST JSON to `url`; return response JSON.
    ///
    /// # Errors
    /// Transport failures.
    fn post_json(&self, url: &str, body: &Value) -> Result<Value, AmError>;

    /// POST JSON to `url` with an optional bearer credential; return response JSON.
    ///
    /// Default delegates to [`Transport::post_json`] without the credential, so
    /// existing transports keep working; transports that can carry the header
    /// override this.
    ///
    /// # Errors
    /// Transport failures.
    fn post_json_bearer(
        &self,
        url: &str,
        body: &Value,
        bearer: Option<&str>,
    ) -> Result<Value, AmError> {
        let _ = bearer;
        self.post_json(url, body)
    }
}

/// Global request deadline for the live transport.
pub const DEFAULT_EXTRACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Live extractor transport over the shared HTTPS client (bounded cap, verified TLS).
///
/// Reuses the embed sidecar's ureq transport: verified TLS for `https://`,
/// plain `http://` for local sidecars, 64 MiB response cap, and no redirect
/// following. It never logs bodies or keys.
#[derive(Debug)]
pub struct HttpTransport {
    /// Global request deadline.
    pub timeout: std::time::Duration,
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_EXTRACT_TIMEOUT,
        }
    }
}

impl Transport for HttpTransport {
    fn post_json(&self, url: &str, body: &Value) -> Result<Value, AmError> {
        self.post_json_bearer(url, body, None)
    }

    fn post_json_bearer(
        &self,
        url: &str,
        body: &Value,
        bearer: Option<&str>,
    ) -> Result<Value, AmError> {
        clio_index::post_json(url, body, self.timeout, bearer)
    }
}

/// Profile A/B/C-shaped adapter: builds a NuExtract-style template request.
///
/// Product rules after verification do not fork by profile; only endpoint/model do.
#[derive(Debug)]
pub struct TemplateApiExtractor<T: Transport> {
    /// Extract endpoint URL.
    pub url: String,
    /// Model id (Profile C default: `numind/NuExtract-1.5-tiny`).
    pub model: String,
    /// Decoding temperature (Profile C ≈ `0.0`).
    pub temperature: f64,
    /// Injected transport.
    pub transport: T,
}

impl<T: Transport> Extractor for TemplateApiExtractor<T> {
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
        use clio_config::secret::{masked_clone, scrub_inline_secrets};
        // Fail closed at the adapter boundary: this is a public entry point, so
        // a direct caller that bypassed the orchestration path must not be able
        // to egress an empty or unbounded context.
        if let Some(context) = &req.context {
            clio_types::validate_context(context)?;
        }
        // Egress is scrubbed like the hosted chat path: inline `key: secret`
        // pairs leave the source text, secret-looking fields leave the tool
        // payload and retry feedback, and the caller-supplied context leaves
        // as scrubbed untrusted data, before the body crosses the process
        // boundary.
        let body = json!({
            "model": self.model,
            "temperature": self.temperature,
            "template": req.schema_template,
            "text": scrub_inline_secrets(&req.source_text),
            "tool_payload": req.tool_payload.as_ref().map(masked_clone),
            "verify_feedback": req.verify_feedback.as_deref().map(scrub_inline_secrets),
            "context": req.context.as_deref().map(scrub_inline_secrets),
        });
        let resp = self.transport.post_json(&self.url, &body)?;
        // The snapshot is schema-typed: only a JSON object is a candidate
        // (same shape rule as the hosted chat path).
        let snapshot = match resp.get("snapshot") {
            None | Some(Value::Null) => None,
            Some(v) if v.is_object() => Some(v.clone()),
            Some(_) => {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "template extract snapshot must be a JSON object",
                ));
            }
        };
        let gist = resp.get("gist").and_then(Value::as_str).map(str::to_owned);
        let source_ref = resp
            .get("source_ref")
            .and_then(Value::as_str)
            .unwrap_or("extract")
            .to_owned();
        let diagnostic = resp.get("diagnostic").cloned();
        Ok(ExtractCandidate {
            snapshot,
            gist,
            source_ref,
            diagnostic,
        })
    }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod extract_tests;
