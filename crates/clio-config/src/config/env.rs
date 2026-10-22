// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//

//! # Responsibility
//!
//! This module is responsible for process-env overlay wiring on `Runtime`
//! (`CLIO_*` variables) and the bearer credential precedence lookup
//! (`bearer_for`).
//!
//! ## Owns
//! - `Runtime::apply_process_env` (the injectable process-env overlay)
//! - `Runtime::bearer_for` credential precedence
//!   (`credentials.<service>_api_key` over `credentials.api_key`)
//!
//! ## Does not own
//! - Overlay flatten/set/merge primitives (see `merge`)
//! - Env variable validation rules (see `validate`)
//! - Other `Runtime` tool handlers (see `config`)
//!
//! ## Boundary
//!
//! Changes to which env variables map to which config paths, or to bearer
//! credential precedence, should normally require changes here. Changes to
//! overlay merge mechanics should not require changes here.
//!
//! Keep this module focused on env overlay wiring and bearer lookup.

use super::Runtime;
use crate::db_url_env::database_url_from;
use serde_json::Value;

impl Runtime {
    /// Apply process-env style overrides via an injectable lookup (one codegen path for coverage).
    ///
    /// Every variable read here carries the `CLIO_` prefix. A name without that
    /// prefix is not a second spelling: the lookup finds nothing, the config
    /// default stands, and nothing is printed to say so.
    pub(crate) fn apply_process_env(&mut self, get: &mut dyn FnMut(&str) -> Option<String>) {
        self.env_overlay.clear();
        if let Some(level) = get("CLIO_LOG_LEVEL") {
            self.env_overlay
                .insert("log.level".into(), Value::String(level));
        }
        if let Some(backend) = get("CLIO_BACKEND") {
            self.env_overlay
                .insert("backend".into(), Value::String(backend));
        }
        if let Some(url) = database_url_from(&mut *get) {
            self.env_overlay
                .insert("store.database_url".into(), Value::String(url));
        }
        if let Some(url) = get("CLIO_EMBED_URL") {
            self.env_overlay
                .insert("embed.url".into(), Value::String(url));
        }
        if let Some(model) = get("CLIO_EMBED_MODEL") {
            self.env_overlay
                .insert("embed.model".into(), Value::String(model));
        }
        if let Some(provider) = get("CLIO_EMBED_PROVIDER") {
            self.env_overlay
                .insert("embed.provider".into(), Value::String(provider));
        }
        if let Some(dims) = get("CLIO_EMBED_DIMS").and_then(|d| d.parse::<u64>().ok()) {
            self.env_overlay
                .insert("embed.dims".into(), Value::from(dims));
        }
        if let Some(key) = get("CLIO_EMBED_API_KEY") {
            self.env_overlay
                .insert("credentials.embed_api_key".into(), Value::String(key));
        }
        if let Some(url) = get("CLIO_RERANK_URL") {
            self.env_overlay
                .insert("rerank.url".into(), Value::String(url));
        }
        if let Some(model) = get("CLIO_RERANK_MODEL") {
            self.env_overlay
                .insert("rerank.model".into(), Value::String(model));
        }
        if let Some(provider) = get("CLIO_RERANK_PROVIDER") {
            self.env_overlay
                .insert("rerank.provider".into(), Value::String(provider));
        }
        if let Some(max_client_batch) =
            get("CLIO_RERANK_MAX_CLIENT_BATCH").and_then(|value| value.parse::<u64>().ok())
        {
            self.env_overlay.insert(
                "rerank.max_client_batch".into(),
                Value::from(max_client_batch),
            );
        }
        if let Some(key) = get("CLIO_RERANK_API_KEY") {
            self.env_overlay
                .insert("credentials.rerank_api_key".into(), Value::String(key));
        }
        if let Some(provider) = get("CLIO_EXTRACT_PROVIDER") {
            self.env_overlay
                .insert("extract.provider".into(), Value::String(provider));
        }
        if let Some(url) = get("CLIO_EXTRACT_URL") {
            self.env_overlay
                .insert("extract.url".into(), Value::String(url));
        }
        if let Some(model) = get("CLIO_EXTRACT_MODEL") {
            self.env_overlay
                .insert("extract.model".into(), Value::String(model));
        }
        if let Some(key) = get("CLIO_EXTRACT_API_KEY") {
            self.env_overlay
                .insert("credentials.extract_api_key".into(), Value::String(key));
        }
        if let Some(bank) = get("CLIO_BANK") {
            self.env_overlay
                .insert("bank.fixed".into(), Value::String(bank));
        }
        if let Some(root) = get("CLIO_REPO_ROOT") {
            self.repo_root = Some(root);
        }
    }

    /// Bearer credential for an external service (precedence):
    /// `credentials.<service>_api_key` wins over the generic `credentials.api_key`;
    /// empty strings count as absent. Pure lookup — never logged.
    #[must_use]
    pub fn bearer_for(&self, service: &str) -> Option<String> {
        bearer_for_effective(&self.effective_raw(), service)
    }
}

/// Bearer credential for an external service from an effective config document.
///
/// Same precedence as [`Runtime::bearer_for`] (`credentials.<service>_api_key`
/// over `credentials.api_key`; empty counts as absent), so callers that already
/// hold a resolved effective config do not duplicate the rule or read the
/// environment. Never logged.
#[must_use]
pub fn bearer_for_effective(effective: &Value, service: &str) -> Option<String> {
    let pick = |pointer: &str| {
        effective
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|s| !s.is_empty())
    };
    pick(&format!("/credentials/{service}_api_key")).or_else(|| pick("/credentials/api_key"))
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
