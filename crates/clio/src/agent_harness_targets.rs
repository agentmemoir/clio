// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving where a direct-write harness's
//! config file lives on this machine, and the entry shape that file uses:
//! one descriptor per harness, computed from an injectable environment.
//!
//! ## Owns
//! - The `DirectWriteConfig` descriptor: the config file path, the top-level
//!   key that holds server entries, and the entry `type` field
//! - The `DirectWriteEnv` seam: the OS layout and the directories the
//!   layouts read, injectable for tests
//! - The per-harness resolution: Cursor's home-joined global config, and
//!   GitHub Copilot's VS Code profile file with its Copilot fallback file
//!   used only when the profile cannot be resolved
//!
//! ## Does not own
//! - The harness records, their mechanisms, and name resolution (see
//!   `agent_harness`)
//! - Writing, parsing, or merging any config file (see `agent_harness_write`
//!   and the install flow)
//!
//! ## Boundary
//!
//! Changes to where a direct-write harness keeps its config, or how its
//! entries are shaped, should normally require changes here. Changes to
//! which harnesses exist, or how a config file is written, should not.
//!
//! Keep this module focused on direct-write config resolution.

use std::path::{Path, PathBuf};

/// The JSON config file a direct-write harness reads, and the entry shape
/// Clio writes into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectWriteConfig {
    /// Path of the JSON config file Clio writes.
    pub path: PathBuf,
    /// Top-level key the harness reads server entries from.
    pub top_level_key: &'static str,
    /// The `type` field each entry carries, when the harness wants one.
    pub entry_type: Option<&'static str>,
}

/// Which OS layout resolves the VS Code user-profile directory: macOS under
/// `~/Library/Application Support`, Windows under `%APPDATA%`, Linux and
/// other Unix under `$XDG_CONFIG_HOME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OsLayout {
    Mac,
    Windows,
    Linux,
}

/// Where a direct-write harness's config files live: the OS layout, the home
/// directory, and the config roots the layouts use. Injectable for tests,
/// like `DelegateEnv`.
pub(crate) struct DirectWriteEnv {
    os: OsLayout,
    home: PathBuf,
    xdg_config_home: Option<PathBuf>,
    appdata: Option<PathBuf>,
    copilot_home: Option<PathBuf>,
}

impl DirectWriteEnv {
    /// The real machine: directories from the process environment, the OS
    /// layout from the target platform.
    pub(crate) fn production() -> DirectWriteEnv {
        DirectWriteEnv {
            os: if cfg!(target_os = "macos") {
                OsLayout::Mac
            } else if cfg!(target_os = "windows") {
                OsLayout::Windows
            } else {
                OsLayout::Linux
            },
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_default(),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
            copilot_home: std::env::var_os("COPILOT_HOME").map(PathBuf::from),
        }
    }

    /// An injected environment, so tests never touch the real process
    /// environment. Test-only: production always uses [`production`].
    #[cfg(test)]
    pub(crate) fn with(
        os: OsLayout,
        home: PathBuf,
        xdg_config_home: Option<PathBuf>,
        appdata: Option<PathBuf>,
        copilot_home: Option<PathBuf>,
    ) -> DirectWriteEnv {
        DirectWriteEnv {
            os,
            home,
            xdg_config_home,
            appdata,
            copilot_home,
        }
    }

    /// A directory under the home, when the home is known.
    fn under_home(&self, relative: &str) -> Option<PathBuf> {
        (!self.home.as_os_str().is_empty()).then(|| self.home.join(relative))
    }

    /// The Copilot fallback directory: `COPILOT_HOME` when set, else the
    /// `.copilot` directory under the home.
    fn copilot_fallback_dir(&self) -> Option<PathBuf> {
        self.copilot_home
            .clone()
            .or_else(|| self.under_home(".copilot"))
    }
}

/// The config file the named direct-write harness uses on this machine.
/// `None` when the harness is unknown here or no file can be resolved.
pub(crate) fn direct_write_config(name: &str, env: &DirectWriteEnv) -> Option<DirectWriteConfig> {
    match name {
        "cursor" => env.under_home(".cursor").map(|dir| DirectWriteConfig {
            path: dir.join("mcp.json"),
            top_level_key: "mcpServers",
            entry_type: None,
        }),
        "github copilot" => copilot_config(env),
        _ => None,
    }
}

/// The GitHub Copilot config file: the VS Code user-profile `mcp.json` when
/// this machine resolves it, otherwise the Copilot fallback file under
/// `COPILOT_HOME` or `~/.copilot`. The two files differ in top-level key and
/// entry shape, so the choice is part of the descriptor.
fn copilot_config(env: &DirectWriteEnv) -> Option<DirectWriteConfig> {
    let (path, top_level_key, entry_type) = match vscode_profile_dir(env) {
        Some(dir) => (dir.join("mcp.json"), "servers", Some("stdio")),
        None => (
            env.copilot_fallback_dir()?.join("mcp-config.json"),
            "mcpServers",
            Some("local"),
        ),
    };
    Some(DirectWriteConfig {
        path,
        top_level_key,
        entry_type,
    })
}

/// The VS Code user-profile directory that holds `mcp.json`, per OS layout.
/// `None` when the layout's directories cannot be resolved, which sends
/// GitHub Copilot to its fallback file.
fn vscode_profile_dir(env: &DirectWriteEnv) -> Option<PathBuf> {
    let code_user = Path::new("Code").join("User");
    let base = match env.os {
        OsLayout::Mac => env.under_home("Library/Application Support")?,
        OsLayout::Linux => env
            .xdg_config_home
            .clone()
            .or_else(|| env.under_home(".config"))?,
        OsLayout::Windows => env
            .appdata
            .clone()
            .or_else(|| env.under_home("AppData/Roaming"))?,
    };
    Some(base.join(code_user))
}

#[cfg(test)]
#[path = "agent_harness_targets_tests.rs"]
mod tests;
