// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for resolving the configured `Extractor` from
//! effective config values.
//!
//! ## Owns
//! - `build_extractor_from`: provider dispatch (`template` | `openai`) over
//!   `extract.provider` with fail-closed unknown-provider and missing
//!   url/model/key handling
//! - The wiring seam the live ingest runtime (later phases) calls with
//!   `runtime.effective_raw()` + `runtime.bearer_for("extract")`
//!
//! ## Does not own
//! - Provider adapters (`extract` / `extract_chat`)
//! - Config schema, allowlist, or env overlays (`clio-config`)
//! - Live ingest wiring into the MCP runtime (owned elsewhere)
//!
//! ## Boundary
//!
//! Changes to which extraction providers exist or how they are selected should
//! normally require changes here. Changes to a provider's request shape should not.
//!
//! Keep this module focused on extraction provider selection.

use crate::extract::{Extractor, TemplateApiExtractor, Transport};
use crate::extract_chat::ChatExtractor;
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::sync::Arc;

/// Build the extractor selected by the effective configuration.
///
/// `effective` is the effective config document (callers pass
/// `runtime.effective_raw()`); `bearer` is the credential precedence lookup
/// (`credentials.extract_api_key` over `credentials.api_key`, may be absent).
/// Fail-closed rules: unknown provider values are rejected; an empty provider
/// keeps `template` (the local default, so hosted extraction is opt-in); the
/// hosted `openai` provider requires a non-empty `extract.url`, a non-empty
/// `extract.model`, and a bearer key. No network I/O happens here: the
/// transport is caller-supplied (live or stub), and extraction only egresses
/// when `extract()` is called.
///
/// # Errors
/// Unknown provider, missing hosted url/model/key, or an endpoint the adapter
/// rejects. Messages never contain key material.
pub fn build_extractor_from<T: Transport + 'static>(
    effective: &Value,
    bearer: Option<String>,
    transport: T,
) -> Result<Arc<dyn Extractor>, AmError> {
    let text = |pointer: &str| {
        effective
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let provider = text("/extract/provider");
    let url = text("/extract/url");
    let model = text("/extract/model");
    let temperature = effective
        .pointer("/extract/temperature")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    match provider.as_str() {
        "" | "template" => Ok(Arc::new(TemplateApiExtractor {
            url,
            model,
            temperature,
            transport,
        })),
        "openai" => {
            let key = bearer.ok_or_else(|| {
                AmError::new(
                    ErrorCode::ConfigCorrupt,
                    "hosted extraction requires a key: set credentials.extract_api_key (or credentials.api_key / CLIO_EXTRACT_API_KEY)",
                )
            })?;
            Ok(Arc::new(ChatExtractor::new(
                url,
                model,
                temperature,
                Some(key),
                transport,
            )?))
        }
        other => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "extract.provider must be template|openai (or empty = template), got `{other}`"
            ),
        )),
    }
}

#[cfg(test)]
#[path = "extract_factory_tests.rs"]
mod extract_factory_tests;

#[cfg(test)]
#[path = "extract_factory_config_tests.rs"]
mod extract_factory_config_tests;
