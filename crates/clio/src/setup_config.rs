// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading and writing the deployment config for
//! `clio setup`: reporting existing masked values, validating every chosen field,
//! backing up any prior config, and writing the merged overlay exactly once.
//!
//! ## Owns
//! - Existing deployment-value detection (masked for display)
//! - Validate-all-then-write-once commit of the chosen provider fields
//! - Pruning watched fields the new install type no longer writes
//! - The pre-replace backup hand-off
//!
//! ## Does not own
//! - Which fields an install type chooses (see `setup_wizard`)
//! - Config path resolution, overlay merge order, and path/value validation
//!   (owned by `clio-config`)
//! - Docker orchestration for local services (see `setup_wizard`)
//!
//! ## Boundary
//!
//! Changes to how setup persists its config should normally require changes here.
//! Changes to install-type selection or Docker lifecycle should not require changes here.
//!
//! Keep this module focused on setup's deployment-config persistence.

use clio_config::secret::{mask_secret, path_is_secret};
use clio_config::{ConfigScope, Runtime, backup_config_file};
use clio_types::ToolResult;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Deployment-overlay fields `clio setup` inspects and writes.
///
/// `backend` and `store.database_url` are deliberately adjacent: they are one
/// statement about which store to open, and a plan that writes one without the
/// other is the silent-SQLite bug. `store.sqlite_path` is watched so a switch
/// to Postgres removes the file path the old choice left behind.
pub(crate) const WATCHED_FIELDS: &[&str] = &[
    "backend",
    "store.database_url",
    "store.sqlite_path",
    "embed.provider",
    "embed.url",
    "embed.model",
    "embed.dims",
    "rerank.provider",
    "rerank.url",
    "rerank.model",
    "credentials.embed_api_key",
    "credentials.rerank_api_key",
];

/// One deployment value already present in the config file, masked for display.
#[allow(
    dead_code,
    reason = "kept and tested; the preview reports the same values through the runtime"
)]
pub(crate) struct ExistingValue {
    pub(crate) path: &'static str,
    pub(crate) display: String,
}

/// Outcome of a successful config write.
#[derive(Debug)]
pub(crate) struct Written {
    pub(crate) path: PathBuf,
    /// Where the previous config was copied, when there was one.
    ///
    /// The preview prints this path before the write; tests read it back. The
    /// applied summary names only the count, so the field has no live reader.
    #[allow(
        dead_code,
        reason = "read by the preview's tests and asserted after a write"
    )]
    pub(crate) backup: Option<PathBuf>,
    pub(crate) fields: usize,
}

/// Values the durable deployment overlay already sets, masked. Only fields the
/// file itself sets are reported (merge defaults are not "already configured").
#[allow(
    dead_code,
    reason = "kept and tested; the preview reads the runtime directly"
)]
pub(crate) fn existing_values(rt: &Runtime) -> Vec<ExistingValue> {
    WATCHED_FIELDS
        .iter()
        .filter_map(|path| {
            rt.deployment_field(path).map(|value| ExistingValue {
                path,
                display: masked_display(path, value),
            })
        })
        .collect()
}

/// A secret path shows only its mask; everything else shows its scalar text.
#[allow(dead_code, reason = "helper of existing_values, kept with it")]
fn masked_display(path: &str, value: &Value) -> String {
    let raw = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if path_is_secret(path) {
        mask_secret(&raw)
    } else {
        raw
    }
}

/// Validate every `(field, value)` pair, back up any existing config, then
/// apply and persist the merged deployment overlay exactly once.
///
/// A validation failure returns before any write or backup, so the previous
/// config file is left intact and no half-written config can exist.
///
/// # Errors
/// A human-readable message naming the offending field, the failed backup, or
/// the failed write.
pub(crate) fn write(
    rt: &mut Runtime,
    path: &Path,
    fields: &[(String, Value)],
) -> Result<Written, String> {
    // Stage in memory: `config_set` only persists when a path is set.
    rt.clear_deployment_path();
    // A full setup run replaces the previous install type: drop watched fields
    // the new plan no longer writes so stale credentials never survive a
    // switch (for example hosted API keys left in an airgapped config).
    let kept: Vec<&str> = fields.iter().map(|(field, _)| field.as_str()).collect();
    let stale: Vec<&str> = WATCHED_FIELDS
        .iter()
        .copied()
        .filter(|field| !kept.contains(field))
        .collect();
    rt.remove_deployment_fields(&stale);
    for (field, value) in fields {
        if let ToolResult::Err { message, .. } =
            rt.config_set(field, value.clone(), ConfigScope::Deployment)
        {
            return Err(format!("invalid value for {field}: {message}"));
        }
    }
    let backup = backup_config_file(path).map_err(|e| format!("backup failed: {}", e.message))?;
    rt.set_deployment_path(path);
    rt.flush_deployment_overlay()
        .map_err(|e| format!("failed to write {}: {}", path.display(), e.message))?;
    Ok(Written {
        path: path.to_path_buf(),
        backup,
        fields: fields.len(),
    })
}

#[cfg(test)]
#[path = "setup_config_tests.rs"]
mod tests;
