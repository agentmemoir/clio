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
//!   objects, one part at a time, so a part that is present but unusable costs
//!   only itself and a missing file costs nothing at all
//! - Re-checking every loaded overlay entry against the `config set` path and
//!   value validators, and refusing a secret-bearing path outright, because
//!   this file is hand-writable and seals nothing
//! - The boot order they are restored in: profile, then its overlay, then the
//!   ranking, so an explicit ranking set wins over the profile's own ranking
//! - Writing the current three-part state owner-only, into an owner-only
//!   directory, atomically
//! - Reporting which overlay keys a write had to leave out, because the file
//!   seals nothing and a secret cannot be written there
//!
//! ## Does not own
//! - The ranking schema itself (see `crate::ranking`)
//! - What `config set` accepts (see `validate`)
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
use super::persist::{default_config_dir, restrict_dir, write_private_atomic};
use super::validate::{validate_set_path, validate_set_value};
use crate::error::{AmError, ErrorCode};
use crate::profile::lookup_profile;
use crate::ranking::RankingEnv;
use crate::secret::path_is_secret;
use serde_json::{Map, Value};
use std::fs;
use std::io::ErrorKind;
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

/// The state stored in `path`, or `None` when the file is absent or cannot be
/// read or parsed whole.
///
/// An absent file is the ordinary "nothing saved yet" and says nothing. A file
/// this cannot read or parse warns with its path and the reason, because a
/// silent `None` there loses the active profile and the overlay with no trace.
/// Each part that is present but unusable is dropped on its own, so one bad key
/// never costs the other two.
fn load_state(path: &Path) -> Option<CliState> {
    let shown = path.to_string_lossy().into_owned();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            crate::log::warn(
                "cli state unreadable",
                &[("path", &shown), ("error", &e.to_string())],
            );
            return None;
        }
    };
    let root = match serde_json::from_str::<Value>(&text) {
        Ok(root) => root,
        Err(e) => {
            crate::log::warn(
                "cli state not parsed",
                &[("path", &shown), ("error", &e.to_string())],
            );
            return None;
        }
    };
    if !root.is_object() {
        crate::log::warn(
            "cli state not parsed",
            &[("path", &shown), ("error", "the file is not a JSON object")],
        );
        return None;
    }
    Some(CliState {
        active_profile: load_active_profile(&root, &shown),
        profile_overlay: load_profile_overlay(&root, &shown),
        ranking: load_ranking(&root, &shown),
    })
}

/// The saved active profile, when the file carries a usable name.
///
/// A `null` value is what a flush writes when no profile is active, so it means
/// "none" rather than a defect.
fn load_active_profile(root: &Value, shown: &str) -> Option<String> {
    let value = root.get(ACTIVE_PROFILE_KEY)?;
    if value.is_null() {
        return None;
    }
    if let Some(name) = value.as_str() {
        return Some(name.to_owned());
    }
    crate::log::warn(
        "cli state active profile not loaded",
        &[("path", shown), ("error", "active_profile is not a string")],
    );
    None
}

/// The saved ranking environment, when the file carries one this version reads.
fn load_ranking(root: &Value, shown: &str) -> Option<RankingEnv> {
    let value = root.get(RANKING_KEY)?;
    if value.is_null() {
        return None;
    }
    match serde_json::from_value::<RankingEnv>(value.clone()) {
        Ok(ranking) => Some(ranking),
        Err(e) => {
            crate::log::warn(
                "cli state ranking not loaded",
                &[("path", shown), ("error", &e.to_string())],
            );
            None
        }
    }
}

/// The saved profile overlay, keeping only the entries `config set` would have
/// accepted and dropping the rest with a warning naming the key and the reason.
///
/// This file is hand-writable and seals nothing, so it must not become a second,
/// unchecked write channel into the effective configuration: an entry is read
/// back only after the same path and value checks a `config set` call passes,
/// and a secret-bearing path is refused even when its value is well typed.
fn load_profile_overlay(root: &Value, shown: &str) -> Overlay {
    let Some(value) = root.get(PROFILE_OVERLAY_KEY) else {
        return Overlay::new();
    };
    let Some(object) = value.as_object() else {
        crate::log::warn(
            "cli state overlay not loaded",
            &[
                ("path", shown),
                ("error", "profile_overlay is not an object"),
            ],
        );
        return Overlay::new();
    };
    let mut overlay = Overlay::new();
    for (key, value) in object {
        match checked_overlay_entry(key, value) {
            Ok(()) => {
                overlay.insert(key.clone(), value.clone());
            }
            Err(e) => crate::log::warn(
                "cli state overlay entry dropped",
                &[("path", shown), ("key", key), ("reason", &e.message)],
            ),
        }
    }
    overlay
}

/// The check one saved overlay entry must pass to be restored.
fn checked_overlay_entry(key: &str, value: &Value) -> Result<(), AmError> {
    if path_is_secret(key) {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "a secret cannot be stored in the unsealed cli state file",
        ));
    }
    validate_set_path(key)?;
    validate_set_value(key, value)
}

/// Write `state` as the file's keys, into an owner-only directory.
fn save_state(path: &Path, state: &CliState) -> Result<(), AmError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| {
            AmError::new(
                ErrorCode::Internal,
                format!("failed to create {}: {e}", parent.display()),
            )
        })?;
        restrict_dir(parent);
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
