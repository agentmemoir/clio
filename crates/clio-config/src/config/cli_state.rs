// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable CLI state file: resolving its
//! path beside the deployment overlay, reading the ranking environment a
//! previous command saved, and writing the current one back atomically.
//!
//! ## Owns
//! - The `cli-state.json` file name and its path resolution under the clio
//!   config directory
//! - Reading the stored `ranking` object, ignoring a missing or corrupt file
//! - Writing the current ranking environment owner-only and atomically
//!
//! ## Does not own
//! - The ranking schema itself (see `crate::ranking`)
//! - The deployment overlay file or its sealed values (see `persist`)
//! - Deciding when to flush (the CLI layer owns that; the tool handlers stay
//!   in memory, because the MCP server shares them)
//!
//! ## Boundary
//!
//! Changes to what the CLI state file holds or where it lives belong here.
//! Changes to ranking validation should not require changes here.
//!
//! Keep this module focused on the durable CLI state file.

use super::Runtime;
use super::persist::{default_config_dir, write_private_atomic};
use crate::error::{AmError, ErrorCode};
use crate::ranking::RankingEnv;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Durable CLI state file name under the clio config directory.
pub const CLI_STATE_FILE_NAME: &str = "cli-state.json";

/// The one key the file holds: the full ranking environment.
const RANKING_KEY: &str = "ranking";

/// Resolve the CLI state path: explicit override, else the XDG default under
/// `$XDG_CONFIG_HOME/clio/` (fallback `$HOME/.config/clio/`), else `None` when
/// no config directory is known.
#[must_use]
pub fn resolve_cli_state_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    resolve_cli_state_path_with(explicit, &|key| std::env::var(key).ok())
}

/// Injectable variant of [`resolve_cli_state_path`], so tests never depend on
/// the host environment.
#[must_use]
pub(crate) fn resolve_cli_state_path_with(
    explicit: Option<PathBuf>,
    get: &impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path);
    }
    Some(default_config_dir(get)?.join(CLI_STATE_FILE_NAME))
}

impl Runtime {
    /// Boot with an injected environment lookup and an explicit CLI state file,
    /// and no deployment overlay.
    #[must_use]
    pub fn with_env_and_cli_state_file(
        get: &dyn Fn(&str) -> Option<String>,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self::boot_with(&|key| get(key), None, None, Some(path.into()))
    }

    /// Load the persisted ranking environment over the boot default. A missing,
    /// unreadable, or corrupt file leaves the boot value in place.
    pub(super) fn load_cli_state(&mut self) {
        let Some(path) = self.cli_state_path.clone() else {
            return;
        };
        if let Some(ranking) = load_ranking(&path) {
            self.ranking = ranking;
        }
    }

    /// Write the current ranking environment to the resolved CLI state file,
    /// atomically and owner-only.
    ///
    /// Returns whether a file was written: `Ok(false)` means no path resolved,
    /// so nothing could be saved and the caller must not claim it was.
    ///
    /// # Errors
    /// I/O failures creating parent directories or writing the file.
    pub fn flush_cli_state(&self) -> Result<bool, AmError> {
        let Some(path) = self.cli_state_path.clone() else {
            return Ok(false);
        };
        save_ranking(&path, &self.ranking)?;
        Ok(true)
    }
}

/// The ranking environment stored in `path`, or `None` when the file is
/// missing, unreadable, not JSON, or does not carry one.
fn load_ranking(path: &Path) -> Option<RankingEnv> {
    // A missing or unreadable file is simply "nothing saved yet".
    let text = fs::read_to_string(path).ok()?;
    let ranking = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|root| root.get(RANKING_KEY).cloned())
        .and_then(|value| serde_json::from_value(value).ok());
    if ranking.is_none() {
        crate::log::warn(
            "cli state not loaded",
            &[("path", &path.to_string_lossy())],
        );
    }
    ranking
}

/// Write `ranking` as the file's sole key.
fn save_ranking(path: &Path, ranking: &RankingEnv) -> Result<(), AmError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| {
            AmError::new(
                ErrorCode::Internal,
                format!("failed to create {}: {e}", parent.display()),
            )
        })?;
    }
    let mut root = Map::new();
    root.insert(
        RANKING_KEY.into(),
        serde_json::to_value(ranking).expect("RankingEnv serializes"),
    );
    let text = serde_json::to_string_pretty(&Value::Object(root)).expect("serialize cli state");
    write_private_atomic(path, &text).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to write cli state {}: {e}", path.display()),
        )
    })
}

#[cfg(test)]
#[path = "cli_state_tests.rs"]
mod tests;
