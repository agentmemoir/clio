// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for building the operations embedding sidecar
//! from the effective configuration.
//!
//! ## Owns
//! - `OpsEmbedConfig` (resolved `embed.*` values) and its effective-config reader
//! - `build_ops_embedder` / `build_ops_embedder_from` and their fail-closed rules
//!
//! ## Does not own
//! - Provider adapters or provider selection (`clio-index`)
//! - Effective-config merging, env overlays, or bearer precedence (`clio-config`)
//! - MCP tool dispatch or store opening (`runtime`)
//!
//! ## Boundary
//!
//! Changes to how the ops embedder is resolved from config should normally
//! require changes here. Changes to provider dialects or config merging should not.
//!
//! Keep this module focused on ops embedder construction.

use clio_index::Embedder;
use clio_store::EMBEDDING_DIMS;
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::sync::Arc;

/// The provider an unset `embed.provider` resolves to: the tei sidecar.
///
/// The effective document carries the empty system default for an omitted
/// value, and config validation refuses to store an explicitly empty provider,
/// so the empty read here is the unset representation, not a stored value.
const DEFAULT_EMBED_PROVIDER: &str = "tei";

/// Resolved `embed.*` values for the ops embedder factory.
#[derive(Debug, Clone)]
pub(crate) struct OpsEmbedConfig {
    /// The resolved `embed.provider` (`DEFAULT_EMBED_PROVIDER` when unset).
    pub provider: String,
    /// `embed.url` API/sidecar base URL.
    pub url: String,
    /// `embed.model` model id.
    pub model: String,
    /// `embed.dims` configured output width.
    pub dims: usize,
    /// Bearer credential for the provider, when configured.
    pub bearer: Option<String>,
}

impl OpsEmbedConfig {
    /// Read `embed.*` out of an effective config document.
    pub(crate) fn from_effective(effective: &Value, bearer: Option<String>) -> Self {
        let text = |pointer: &str| {
            effective
                .pointer(pointer)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        let dims = effective
            .pointer("/embed/dims")
            .and_then(Value::as_u64)
            .and_then(|d| usize::try_from(d).ok())
            .unwrap_or(EMBEDDING_DIMS);
        // An unset provider — absent, null, or the empty system default —
        // resolves to the explicit tei default; config validation refuses to
        // store an explicitly empty provider value.
        let stored_provider = text("/embed/provider");
        let provider = if stored_provider.is_empty() {
            DEFAULT_EMBED_PROVIDER.to_owned()
        } else {
            stored_provider
        };
        Self {
            provider,
            url: text("/embed/url"),
            model: text("/embed/model"),
            dims,
            bearer,
        }
    }
}

/// Build the ops embedding sidecar from the effective configuration.
///
/// `Ok(None)` disables live dense reindex (lexical rebuild still works). An
/// unknown provider fails closed with the allowed values instead of silently
/// disabling dense work.
///
/// # Errors
/// Unknown `embed.provider`, or an adapter configuration the provider rejects.
pub fn build_ops_embedder() -> Result<Option<Arc<dyn Embedder>>, AmError> {
    build_ops_embedder_with(&|key| std::env::var(key).ok())
}

/// Build the ops embedding sidecar against an injected environment lookup.
///
/// The same split every other command surface already has: the caller decides
/// where the environment comes from, so a test can say "no deployment config"
/// instead of inheriting whatever the developer's machine has (#90). Without it
/// a `clio ops` test's verdict depended on a real sidecar being configured on
/// the host, which is what made `reindex_dense_confirmed_without_embedder_exits_3`
/// fail on a configured machine and pass on a bare one.
///
/// # Errors
/// Unknown `embed.provider`, or an adapter configuration the provider rejects.
pub fn build_ops_embedder_with<F>(get: &F) -> Result<Option<Arc<dyn Embedder>>, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let runtime = clio_config::Runtime::with_env(get);
    let effective = runtime.effective_raw();
    build_ops_embedder_from(&OpsEmbedConfig::from_effective(
        &effective,
        runtime.bearer_for("embed"),
    ))
}

/// Build the ops embedding sidecar from resolved config values.
///
/// An empty url means "no provider configured" and yields `Ok(None)`. A
/// partially configured provider fails closed with `ConfigCorrupt`: the tei
/// sidecar may use the conventional default model, any other provider requires
/// a non-empty `embed.model`, and an explicit `embed.dims` of 0 is rejected, so
/// an operator mistake never silently disables dense work. An unknown provider
/// or a rejected endpoint surfaces the adapter error on every config path.
///
/// # Errors
/// Unknown provider, a partially configured provider (missing model / zero
/// dims), or an endpoint/model the selected adapter rejects.
pub(crate) fn build_ops_embedder_from(
    cfg: &OpsEmbedConfig,
) -> Result<Option<Arc<dyn Embedder>>, AmError> {
    if cfg.url.trim().is_empty() {
        return Ok(None);
    }
    let model = if cfg.model.is_empty() {
        if cfg.provider == DEFAULT_EMBED_PROVIDER {
            clio_store::DEFAULT_EMBEDDING_MODEL.to_owned()
        } else {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.model is required unless embed.provider is tei (the sidecar default model)",
            ));
        }
    } else {
        cfg.model.clone()
    };
    if cfg.dims == 0 {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "embed.dims must be a positive vector width when embed.url is set",
        ));
    }
    clio_index::build_embedder(
        &cfg.provider,
        cfg.url.clone(),
        model,
        cfg.dims,
        cfg.bearer.clone(),
    )
    .map(Some)
}

#[cfg(test)]
#[path = "ops_embedder_tests.rs"]
mod ops_embedder_tests;
