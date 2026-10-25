// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the durable CLI state file: resolving its
//! path beside the deployment overlay, reading the ranking environment, the
//! active profile, and the profile overlay a previous command saved, and
//! writing all three back atomically.
//!
//! ## Owns
//! - The `cli-state.json` file name and its path resolution under the clio
//!   config directory
//! - Reading the stored `active_profile`, `profile_overlay`, and `ranking`
//!   objects, ignoring a missing or corrupt file
//! - The boot order they are restored in: profile, then its overlay, then the
//!   ranking, so an explicit ranking set wins over the profile's own ranking
//! - Writing the current three-part state owner-only and atomically
//! - Reporting which overlay keys a write had to leave out, because the file
//!   seals nothing and a secret cannot be written there
//!
//! ## Does not own
//! - The ranking schema itself (see `crate::ranking`)
//! - The profile catalog or what applying one sets (see `profile_ops`)
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
use super::merge::Overlay;
use super::persist::{default_config_dir, write_private_atomic};
use crate::error::{AmError, ErrorCode};
use crate::profile::lookup_profile;
use crate::ranking::RankingEnv;
use crate::secret::path_is_secret;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Durable CLI state file name under the clio config directory.
pub const CLI_STATE_FILE_NAME: &str = "cli-state.json";

/// The full ranking environment, which wins over a profile's own ranking.
const RANKING_KEY: &str = "ranking";
/// The profile a previous `clio config use` activated.
const ACTIVE_PROFILE_KEY: &str = "active_profile";
/// The flat key/value overlay a previous `config set --scope profile` wrote.
const PROFILE_OVERLAY_KEY: &str = "profile_overlay";

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

/// One file's worth of durable CLI state; every part is optional, so a file
/// written by an older version boots exactly as before.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct CliState {
    pub(crate) active_profile: Option<String>,
    pub(crate) profile_overlay: Overlay,
    pub(crate) ranking: Option<RankingEnv>,
}

/// What one CLI state write saved: whether a file was written, and the overlay
/// keys the unsealed file could not hold.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CliStateFlush {
    written: bool,
    dropped: Vec<String>,
}

impl CliStateFlush {
    /// Whether a state file was written at all.
    #[must_use]
    pub fn written(&self) -> bool {
        self.written
    }

    /// The overlay keys left out of the file, so a caller cannot report the
    /// whole change as saved.
    #[must_use]
    pub fn dropped(&self) -> &[String] {
        &self.dropped
    }
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

    /// Load the persisted profile, profile overlay, and ranking environment over
    /// the boot defaults. A missing, unreadable, or corrupt file leaves the boot
    /// value in place, and a profile name the catalog no longer has is skipped
    /// rather than failing the boot.
    pub(super) fn load_cli_state(&mut self) {
        let Some(path) = self.cli_state_path.clone() else {
            return;
        };
        let Some(state) = load_state(&path) else {
            return;
        };
        self.restore_profile(state.active_profile.as_deref(), state.profile_overlay);
        if let Some(ranking) = state.ranking {
            self.ranking = ranking;
        }
    }

    /// Apply the persisted profile the way `config use` does, then put the
    /// persisted overlay back on top of it. The overlay belongs to the profile,
    /// so a profile the catalog no longer has leaves it unapplied.
    fn restore_profile(&mut self, name: Option<&str>, overlay: Overlay) {
        let Some(name) = name else {
            return;
        };
        let Some(profile) = lookup_profile(name) else {
            crate::log::warn("cli state profile not found", &[("profile", name)]);
            return;
        };
        // Applying a profile clears the overlay, so it is restored afterwards.
        self.apply_profile(&profile);
        for (key, value) in overlay {
            self.profile_overlay.insert(key, value);
        }
    }

    /// Write the active profile, the profile overlay, and the ranking
    /// environment to the resolved CLI state file, atomically and owner-only.
    ///
    /// A file that cannot be written reports itself: an unwritten file has
    /// [`CliStateFlush::written`] false, and every overlay key the unsealed file
    /// cannot hold is listed in [`CliStateFlush::dropped`] and logged, so no
    /// caller can report the whole change as saved.
    ///
    /// # Errors
    /// I/O failures creating parent directories or writing the file.
    pub fn flush_cli_state(&self) -> Result<CliStateFlush, AmError> {
        let Some(path) = self.cli_state_path.clone() else {
            return Ok(CliStateFlush::default());
        };
        let (state, dropped) = self.saved_state();
        for key in &dropped {
            crate::log::warn("cli state value not saved", &[("key", key)]);
        }
        save_state(&path, &state)?;
        Ok(CliStateFlush {
            written: true,
            dropped,
        })
    }

    /// The state a flush writes, plus the overlay keys it had to leave out.
    fn saved_state(&self) -> (CliState, Vec<String>) {
        let mut dropped = Vec::new();
        let mut profile_overlay = Overlay::new();
        for (key, value) in &self.profile_overlay {
            // A secret cannot be sealed here, so it stays in the profile overlay
            // of the running process and out of the file rather than in the
            // clear. The caller is told, so it cannot report a full save.
            if path_is_secret(key) {
                dropped.push(key.clone());
            } else {
                profile_overlay.insert(key.clone(), value.clone());
            }
        }
        let state = CliState {
            active_profile: self.active_profile.clone(),
            profile_overlay,
            ranking: Some(self.ranking.clone()),
        };
        (state, dropped)
    }
}

/// The state stored in `path`, or `None` when the file is missing, unreadable,
/// not JSON, or carries nothing this version understands.
fn load_state(path: &Path) -> Option<CliState> {
    // A missing or unreadable file is simply "nothing saved yet".
    let text = fs::read_to_string(path).ok()?;
    let root = serde_json::from_str::<Value>(&text).ok()?;
    let state = CliState {
        active_profile: root
            .get(ACTIVE_PROFILE_KEY)
            .and_then(Value::as_str)
            .map(str::to_owned),
        profile_overlay: root
            .get(PROFILE_OVERLAY_KEY)
            .and_then(Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        ranking: root
            .get(RANKING_KEY)
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    };
    if state.ranking.is_none() {
        crate::log::warn("cli state not loaded", &[("path", &path.to_string_lossy())]);
    }
    Some(state)
}

/// Write `state` as the file's keys.
fn save_state(path: &Path, state: &CliState) -> Result<(), AmError> {
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
        ACTIVE_PROFILE_KEY.into(),
        state
            .active_profile
            .as_ref()
            .map_or(Value::Null, |name| Value::String(name.clone())),
    );
    root.insert(
        PROFILE_OVERLAY_KEY.into(),
        Value::Object(
            state
                .profile_overlay
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
    );
    if let Some(ranking) = &state.ranking {
        root.insert(
            RANKING_KEY.into(),
            serde_json::to_value(ranking).expect("RankingEnv serializes"),
        );
    }
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
