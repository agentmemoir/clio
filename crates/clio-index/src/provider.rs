// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for selecting the embedding provider adapter from
//! the configured `embed.provider` value.
//!
//! ## Owns
//! - `EMBED_PROVIDERS` (the accepted provider names)
//! - `build_embedder`: provider dispatch with fail-closed unknown handling
//!
//! ## Does not own
//! - Provider dialects (see `embed` / `openai`)
//! - Effective-config resolution or env overlays (`clio-config` / `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to which providers exist or how they are selected should normally
//! require changes here. Changes to a provider's request shape should not.
//!
//! Keep this module focused on provider selection.

use crate::embed::{Embedder, TeiEmbedder};
use crate::openai::OpenAiEmbedder;
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

/// Providers `embed.provider` accepts.
pub const EMBED_PROVIDERS: &[&str] = &["tei", "openai"];

/// Build the configured embedder adapter.
///
/// Callers pass a resolved provider: the config readers apply the explicit tei
/// default when `embed.provider` is unset, and an explicitly empty value fails
/// closed here instead of selecting a default.
///
/// # Errors
/// Unknown provider (listing the allowed values), or a rejected adapter
/// configuration (empty url/model, zero dims, non-http endpoint).
pub fn build_embedder(
    provider: &str,
    url: impl Into<String>,
    model: impl Into<String>,
    dims: usize,
    bearer: Option<String>,
) -> Result<Arc<dyn Embedder>, AmError> {
    match provider.trim() {
        "tei" => Ok(Arc::new(TeiEmbedder::new(url, model, dims, bearer)?)),
        "openai" => Ok(Arc::new(OpenAiEmbedder::new(url, model, dims, bearer)?)),
        other => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "embed.provider must be {}, got `{other}`",
                EMBED_PROVIDERS.join("|")
            ),
        )),
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod provider_tests;
