// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the OpenAI-compatible embeddings adapter
//! (`POST {url}/v1/embeddings`) used when `embed.provider = "openai"`.
//!
//! ## Owns
//! - `OpenAiEmbedder`: request shape, health probe, and response parsing
//! - Index-ordered (`data[].index`) vector reconstruction and the fail-closed
//!   count/width gates
//!
//! ## Does not own
//! - The `Embedder` contract or TEI adapter (`embed`)
//! - Provider selection (`provider`) or HTTP transport mechanics (`http`)
//!
//! ## Boundary
//!
//! Changes to the OpenAI request/response shape should normally require changes
//! here. Changes to the shared retry/transport policy should not.
//!
//! Keep this module focused on the OpenAI-compatible embeddings dialect.

use crate::embed::{DEFAULT_EMBED_RETRIES, DEFAULT_EMBED_TIMEOUT, Embedder, post_with_retry};
use crate::http::{HttpEndpoint, get_ok};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};
use std::time::Duration;

/// OpenAI-compatible embeddings client (`POST /v1/embeddings`, `GET /v1/models`).
///
/// `url` is the API base URL (for example `https://api.openai.com`); the
/// versioned paths are appended. Responses are parsed by each entry's `index`
/// so the returned order always matches the input order.
#[derive(Debug)]
pub struct OpenAiEmbedder {
    url: String,
    model_id: String,
    dims: usize,
    bearer: Option<String>,
    timeout: Duration,
    retries: u32,
}

impl OpenAiEmbedder {
    /// Build an OpenAI-compatible client from effective config values.
    ///
    /// # Errors
    /// Empty url/model, zero dims, or a non-http(s) endpoint.
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
                "embed.url is not configured; set embed.url or CLIO_EMBED_URL to the API base URL",
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
        HttpEndpoint::parse(&url)?;
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
        let body = json!({
            "input": texts,
            "model": self.model_id,
            "dimensions": self.dims,
        });
        let url = format!("{}/v1/embeddings", self.url.trim_end_matches('/'));
        let value = post_with_retry(
            &url,
            &body,
            self.timeout,
            self.bearer.as_deref(),
            self.retries,
        )?;
        parse_openai_embeddings(&value, texts.len(), self.dims, &self.model_id)
    }
}

impl Embedder for OpenAiEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        self.embed_once(texts)
    }

    fn health(&self) -> Result<(), AmError> {
        get_ok(
            &format!("{}/v1/models", self.url.trim_end_matches('/')),
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

/// Parse an OpenAI-compatible embeddings response, ordered by `data[].index`.
///
/// Fails closed on a missing/empty `data` array, an index outside `0..batch`,
/// a duplicate index, a count mismatch, or a width mismatch: no truncation,
/// padding, or partial acceptance.
fn parse_openai_embeddings(
    value: &Value,
    batch: usize,
    dims: usize,
    model_id: &str,
) -> Result<Vec<Vec<f32>>, AmError> {
    let data = value.get("data").and_then(Value::as_array).ok_or_else(|| {
        AmError::new(
            ErrorCode::Internal,
            "openai embed response has no `data` array",
        )
    })?;
    if data.len() != batch {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!(
                "embed response count {} does not match batch size {batch}",
                data.len()
            ),
        ));
    }
    let mut ordered: Vec<Option<Vec<f32>>> = vec![None; batch];
    for entry in data {
        let index = entry.get("index").and_then(Value::as_u64).ok_or_else(|| {
            AmError::new(
                ErrorCode::Internal,
                "openai embed response entry has no integer `index`",
            )
        })?;
        let slot = ordered
            .get_mut(usize::try_from(index).unwrap_or(usize::MAX))
            .ok_or_else(|| {
                AmError::new(
                    ErrorCode::OutOfRange,
                    format!("openai embed response index {index} is outside 0..{batch}"),
                )
            })?;
        if slot.is_some() {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!("openai embed response repeats index {index}"),
            ));
        }
        let vector: Vec<f32> =
            serde_json::from_value(entry.get("embedding").cloned().unwrap_or(Value::Null))
                .map_err(|e| {
                    AmError::new(
                        ErrorCode::Internal,
                        format!("openai embed response entry {index} is not a float-vector: {e}"),
                    )
                })?;
        if vector.len() != dims {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "embed model {model_id} returned {} dims for input {index}; expected {dims} (dim mismatch fails closed; re-configure embed.dims and rebuild)",
                    vector.len()
                ),
            ));
        }
        *slot = Some(vector);
    }
    ordered
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::OutOfRange,
                format!("openai embed response is missing an entry for one of 0..{batch}"),
            )
        })
}

#[cfg(test)]
#[path = "openai_tests.rs"]
mod openai_tests;

#[cfg(test)]
#[path = "openai_config_tests.rs"]
mod openai_config_tests;
