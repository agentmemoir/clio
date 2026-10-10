// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the registry of agent harnesses that Clio
//! can install its MCP server into: one record per harness, name
//! normalization, and the pure argument builders for each harness.
//!
//! ## Owns
//! - The harness record table and the supported-name list
//! - Case-insensitive name resolution over the canonical names and aliases
//! - The accepted-name set: the canonical names, `VS Code`, and `claude-code`
//! - The MCP server name Clio registers under
//! - The server argv builder (`<exe> mcp stdio`) and the vendor add/remove
//!   argv builders and manual install snippet per harness
//! - The user-scope config file name and read-back shape of each harness
//! - The direct-write mechanism and the per-record lookup that resolves a
//!   direct-write harness's config file target (the resolution itself, and
//!   its injectable environment, live in `agent_harness_targets`)
//!
//! ## Does not own
//! - Running any harness command or writing any harness config file; the
//!   install flow that consumes this registry does that
//! - Resolving the Clio executable path; callers pass it in
//!
//! ## Boundary
//!
//! Changes to which harnesses are supported, their names, their command
//! shapes, their config file names, or their read-back shape should normally
//! require changes here. Changes to how those commands run or how install
//! results are reported should not require changes here.
//!
//! Keep this module focused on harness registry data and pure builders.

use crate::agent_harness_targets::{DirectWriteConfig, DirectWriteEnv};
use std::ffi::OsString;
use std::path::Path;

/// The MCP server name Clio registers under in every harness.
pub(crate) const SERVER_NAME: &str = "clio";

/// How a harness receives the Clio MCP server registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mechanism {
    /// Clio runs the harness's own CLI to add or remove the registration.
    Delegate,
    /// Clio writes the harness's JSON config file itself.
    DirectWrite,
}

/// How the install flow checks that a harness config names the Clio server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadBack {
    /// A JSON config. The server entry is looked up under each key prefix in
    /// order, with the server name appended; the first prefix that names it
    /// wins. The workspace pins no TOML/YAML/JSONC parser.
    Json {
        key_prefixes: &'static [&'static [&'static str]],
    },
    /// A TOML config with a `[mcp_servers.<name>]` table, checked as text.
    TomlServerTable,
    /// A YAML config with the server under a top-level `mcp_servers:` map,
    /// checked as text.
    YamlServerMap,
}

/// One supported agent harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HarnessRecord {
    /// Canonical name, as shown by the supported-name list.
    pub name: &'static str,
    /// Extra accepted names that resolve to this record.
    pub aliases: &'static [&'static str],
    pub mechanism: Mechanism,
    /// File name of the harness's user-scope config, joined to the user's
    /// home directory when the install flow reads the config back. A
    /// direct-write harness resolves its real file per machine; this is its
    /// home-relative fallback.
    pub config_file: &'static str,
    /// How the install flow reads the server entry back from the config file.
    pub read_back: ReadBack,
}

static RECORDS: &[HarnessRecord] = &[
    HarnessRecord {
        name: "claude",
        aliases: &["claude-code"],
        mechanism: Mechanism::Delegate,
        config_file: ".claude.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["mcpServers"]],
        },
    },
    HarnessRecord {
        name: "codex",
        aliases: &[],
        mechanism: Mechanism::Delegate,
        config_file: ".codex/config.toml",
        read_back: ReadBack::TomlServerTable,
    },
    HarnessRecord {
        name: "opencode",
        aliases: &[],
        mechanism: Mechanism::Delegate,
        config_file: ".config/opencode/opencode.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["mcp", "servers"], &["mcp"]],
        },
    },
    HarnessRecord {
        name: "antigravity",
        aliases: &[],
        mechanism: Mechanism::Delegate,
        config_file: ".gemini/config/mcp_config.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["mcpServers"]],
        },
    },
    HarnessRecord {
        name: "hermes",
        aliases: &[],
        mechanism: Mechanism::Delegate,
        config_file: ".hermes/config.yaml",
        read_back: ReadBack::YamlServerMap,
    },
    HarnessRecord {
        name: "cursor",
        aliases: &[],
        mechanism: Mechanism::DirectWrite,
        config_file: ".cursor/mcp.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["mcpServers"]],
        },
    },
    HarnessRecord {
        name: "github copilot",
        aliases: &[],
        mechanism: Mechanism::DirectWrite,
        config_file: ".copilot/mcp-config.json",
        read_back: ReadBack::Json {
            key_prefixes: &[&["servers"], &["mcpServers"]],
        },
    },
];

/// Canonical names accepted by name matching, each with a record above.
static CANONICAL_NAMES: &[&str] = &[
    "hermes",
    "claude",
    "antigravity",
    "codex",
    "cursor",
    "github copilot",
    "opencode",
];

/// Shared accepted aliases, each mapping to a canonical name.
static NAME_ALIASES: &[(&str, &str)] = &[("vs code", "github copilot"), ("claude-code", "claude")];

/// Resolve an accepted name to its canonical form, case-insensitively: the
/// canonical names first, then the shared aliases, then each record's own
/// aliases. Returns `None` for a name outside the accepted set.
pub(crate) fn normalize(name: &str) -> Option<&'static str> {
    if let Some(canonical) = CANONICAL_NAMES
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
    {
        return Some(canonical);
    }
    if let Some((_, canonical)) = NAME_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
    {
        return Some(canonical);
    }
    RECORDS.iter().find_map(|record| {
        record
            .aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(name))
            .then_some(record.name)
    })
}

/// Resolve a harness name to its implemented record, case-insensitively,
/// aliases included.
pub(crate) fn resolve(name: &str) -> Option<&'static HarnessRecord> {
    let canonical = normalize(name)?;
    RECORDS.iter().find(|record| record.name == canonical)
}

/// Canonical names of every supported harness, in registry order.
pub(crate) fn supported_names() -> Vec<&'static str> {
    RECORDS.iter().map(|record| record.name).collect()
}

/// Argv the harness runs to start the Clio MCP server: the executable
/// followed by `mcp stdio`. Independent of the record, because every
/// supported harness launches the server the same way.
pub(crate) fn server_argv(exe: &Path) -> Vec<OsString> {
    vec![
        exe.as_os_str().to_os_string(),
        OsString::from("mcp"),
        OsString::from("stdio"),
    ]
}

impl HarnessRecord {
    /// Vendor CLI argv that adds the Clio MCP server to this harness.
    /// `argv[0]` is the harness command; the caller spawns it as-is.
    /// A harness without a registered vendor command builds an empty argv,
    /// so the caller refuses instead of spawning anything.
    pub(crate) fn add_argv(&self, server_name: &str, exe: &Path) -> Vec<OsString> {
        match self.name {
            "claude" => vec![
                OsString::from("claude"),
                OsString::from("mcp"),
                OsString::from("add"),
                OsString::from("--transport"),
                OsString::from("stdio"),
                OsString::from("--scope"),
                OsString::from("user"),
                OsString::from(server_name),
                OsString::from("--"),
                exe.as_os_str().to_os_string(),
                OsString::from("mcp"),
                OsString::from("stdio"),
            ],
            "codex" => vec![
                OsString::from("codex"),
                OsString::from("mcp"),
                OsString::from("add"),
                OsString::from(server_name),
                OsString::from("--"),
                exe.as_os_str().to_os_string(),
                OsString::from("mcp"),
                OsString::from("stdio"),
            ],
            "opencode" => vec![
                OsString::from("opencode"),
                OsString::from("mcp"),
                OsString::from("add"),
                OsString::from("--global"),
                OsString::from(server_name),
                OsString::from("--"),
                exe.as_os_str().to_os_string(),
                OsString::from("mcp"),
                OsString::from("stdio"),
            ],
            "antigravity" => vec![
                OsString::from("agy"),
                OsString::from("mcp"),
                OsString::from("add"),
                OsString::from(server_name),
                exe.as_os_str().to_os_string(),
                OsString::from("mcp"),
                OsString::from("stdio"),
            ],
            "hermes" => vec![
                OsString::from("hermes"),
                OsString::from("mcp"),
                OsString::from("add"),
                OsString::from(server_name),
                OsString::from("--command"),
                exe.as_os_str().to_os_string(),
                OsString::from("--args"),
                OsString::from("mcp"),
                OsString::from("stdio"),
            ],
            _ => Vec::new(),
        }
    }

    /// Vendor CLI argv that removes the Clio MCP server from this harness.
    pub(crate) fn remove_argv(&self, server_name: &str) -> Vec<OsString> {
        match self.name {
            "claude" => vec![
                OsString::from("claude"),
                OsString::from("mcp"),
                OsString::from("remove"),
                OsString::from(server_name),
                OsString::from("--scope"),
                OsString::from("user"),
            ],
            "codex" => vec![
                OsString::from("codex"),
                OsString::from("mcp"),
                OsString::from("remove"),
                OsString::from(server_name),
            ],
            "antigravity" => vec![
                OsString::from("agy"),
                OsString::from("mcp"),
                OsString::from("remove"),
                OsString::from(server_name),
            ],
            "hermes" => vec![
                OsString::from("hermes"),
                OsString::from("mcp"),
                OsString::from("remove"),
                OsString::from(server_name),
            ],
            _ => Vec::new(),
        }
    }

    /// One-line manual install command for this harness, built from the same
    /// argv the delegate flow runs. Elements with spaces are quoted.
    pub(crate) fn manual_snippet(&self, server_name: &str, exe: &Path) -> String {
        snippet_from_argv(&self.add_argv(server_name, exe))
    }

    /// The config file Clio writes directly for this harness, resolved for
    /// this machine, with its top-level key and entry shape. `None` for a
    /// delegate harness or when no file can be resolved.
    pub(crate) fn direct_write_config(&self, env: &DirectWriteEnv) -> Option<DirectWriteConfig> {
        if self.mechanism == Mechanism::DirectWrite {
            crate::agent_harness_targets::direct_write_config(self.name, env)
        } else {
            None
        }
    }
}

/// One-line rendering of a command argv. Elements with spaces are quoted.
pub(crate) fn snippet_from_argv(argv: &[OsString]) -> String {
    argv.iter()
        .map(|arg| {
            let word = arg.to_string_lossy();
            if word.contains(' ') {
                format!("\"{word}\"")
            } else {
                word.into_owned()
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
#[path = "agent_harness_tests.rs"]
mod tests;
