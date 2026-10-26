// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the deployment overlay path and the
//! shared config directory from an explicit value or the environment.
//!
//! ## Owns
//! - The `CLIO_DEPLOYMENT_CONFIG` override and its empty-value disable rule
//! - The XDG default directory (`$XDG_CONFIG_HOME`, fallback `$HOME/.config`)
//! - The default deployment overlay file name
//! - Whether a runtime resolved a deployment path at all, which a caller needs
//!   before promising a deployment-scoped value will be saved
//!
//! ## Does not own
//! - Reading or writing the overlay file (see `load`, `save`)
//! - Overlay flatten/set primitives (see `super::merge`)
//!
//! ## Boundary
//!
//! Changes to deployment path resolution should normally require changes here.
//! Changes to the overlay file contents should not require changes here.
//!
//! Keep this module focused on resolving deployment and config paths.

use crate::config::Runtime;
use std::path::{Path, PathBuf};

impl Runtime {
    /// Whether a durable deployment overlay path resolved for this runtime.
    ///
    /// The path itself stays private: a caller only needs to know whether a
    /// deployment-scoped value has somewhere to go.
    #[must_use]
    pub fn has_deployment_path(&self) -> bool {
        self.deployment_path.is_some()
    }
}

/// Env var naming the durable deployment overlay file.
pub const DEPLOYMENT_CONFIG_ENV: &str = "CLIO_DEPLOYMENT_CONFIG";

/// Default deployment overlay file name under the XDG config app directory.
pub const DEPLOYMENT_CONFIG_FILE_NAME: &str = "deployment.json";

/// Resolve the deployment config path: explicit override, else
/// `CLIO_DEPLOYMENT_CONFIG` (an empty value disables loading), else the XDG
/// default under `$XDG_CONFIG_HOME/clio/` (fallback `$HOME/.config/clio/`), else
/// `None` when no home directory is known.
#[must_use]
pub fn resolve_deployment_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    resolve_deployment_path_with(explicit, |key| std::env::var(key).ok())
}

/// Injectable variant of [`resolve_deployment_path`]: the lookup drives both the
/// `CLIO_DEPLOYMENT_CONFIG` override and the XDG default, so tests never depend on
/// the host environment.
#[must_use]
pub(crate) fn resolve_deployment_path_with(
    explicit: Option<PathBuf>,
    get: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    match get(DEPLOYMENT_CONFIG_ENV) {
        // An explicitly set-but-empty override disables deployment loading.
        Some(raw) if raw.trim().is_empty() => None,
        Some(raw) => Some(PathBuf::from(raw.trim())),
        None => default_deployment_path(&get),
    }
}

/// The XDG default deployment path, or `None` when neither `XDG_CONFIG_HOME`
/// nor `HOME` names a usable directory. Relative base directories are ignored
/// (XDG requires absolute paths).
#[must_use]
fn default_deployment_path(get: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(default_config_dir(get)?.join(DEPLOYMENT_CONFIG_FILE_NAME))
}

/// The XDG config base directory (`$XDG_CONFIG_HOME/clio/`, fallback
/// `$HOME/.config/clio/`), or `None` when neither names an absolute path.
/// Shared by every store file that lives under the config directory.
#[must_use]
pub fn default_config_dir(get: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = nonempty(get("XDG_CONFIG_HOME"))
        .filter(|dir| Path::new(dir).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            nonempty(get("HOME"))
                .filter(|home| Path::new(home).is_absolute())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join(crate::db_path::APP_DIR_NAME))
}

/// Trimmed, non-empty lookup value.
fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_owned())
        .filter(|v| !v.is_empty())
}
