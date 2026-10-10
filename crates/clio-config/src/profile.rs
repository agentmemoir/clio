// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for named deployment profiles and coding-agent tool inventory packaging.
//!
//! ## Owns
//! - Built-in profiles (`coding_local`, `coding_ci`) and lookup
//! - Machine-readable Core/Additive inventory with `implemented` flags
//!
//! ## Does not own
//! - Applying overlays into effective config
//! - The set of tools the build binds (`clio_types::mcp_tools`)
//!
//! ## Boundary
//!
//! Changes to profile packaging and declared tool surface should normally require changes here.
//! Changes to config merge or ranking math should not require changes here.
//!
//! Keep this module focused on named profiles and tool-inventory packaging.

use crate::context::BankDefaultPolicy;
use crate::ranking::RankingEnv;
use serde::{Deserialize, Serialize};

/// Whether a catalogued tool has a real handler in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolImplStatus {
    Implemented,
    Stub,
}

/// One entry in the machine-readable tool surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInventoryEntry {
    pub name: &'static str,
    pub group: &'static str,
    pub implemented: bool,
    pub status: ToolImplStatus,
}

/// Named deployment profile.
///
/// Profiles are built in code and looked up by name; JSON input does not
/// deserialize a whole `Profile`.
#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    pub name: String,
    pub description: String,
    /// Coding-agent single-host: sync tools omitted from mandatory Additive set.
    pub single_host: bool,
    pub omit_sync: bool,
    pub bank_default: BankDefaultPolicy,
    pub backend_hint: String,
    pub log_level: String,
    pub injection_budget_tokens: u32,
    pub ranking: RankingEnv,
    /// Normative MCP remote transport note for later slices (stdio + Streamable HTTP).
    pub mcp_transports: Vec<String>,
    pub mcp_revision: String,
    /// The declared gates are NOT satisfied by packaging alone.
    pub fr10_satisfied: bool,
}

impl Profile {
    #[must_use]
    pub fn coding_local() -> Self {
        Self {
            name: "coding_local".into(),
            description: "Local coding-agent profile: per-repo bank defaults, Core+Additive inventory declared; sync omitted (single-host).".into(),
            single_host: true,
            omit_sync: true,
            bank_default: BankDefaultPolicy::PerRepository,
            backend_hint: "sqlite".into(),
            log_level: "info".into(),
            injection_budget_tokens: 2048,
            ranking: RankingEnv::system_defaults(),
            mcp_transports: vec![
                "stdio".into(),
                "streamable_http".into(),
            ],
            // Legacy HTTP+SSE is compatibility-only; not listed as primary.
            mcp_revision: "2025-11-25".into(),
            fr10_satisfied: false,
        }
    }

    #[must_use]
    pub fn coding_ci() -> Self {
        let mut p = Self::coding_local();
        p.name = "coding_ci".into();
        p.description =
            "CI coding-agent profile: same bank policy as coding_local; quieter logs.".into();
        p.log_level = "warn".into();
        p
    }
}

/// All built-in profiles.
#[must_use]
pub fn builtin_profiles() -> Vec<Profile> {
    vec![Profile::coding_local(), Profile::coding_ci()]
}

#[must_use]
pub fn lookup_profile(name: &str) -> Option<Profile> {
    builtin_profiles().into_iter().find(|p| p.name == name)
}

/// Core + Additive names for a coding profile declaration.
///
/// Each entry's `implemented` flag is read from the tool names the build binds
/// over MCP, so the report states what this build can call rather than what the
/// packaging once declared as a stub.
#[must_use]
pub fn coding_tool_inventory(omit_sync: bool) -> Vec<ToolInventoryEntry> {
    let mut entries = Vec::new();
    let core: &[&str] = &[
        "admit_preview",
        "admit_preview_batch",
        "store",
        "get",
        "get_snapshot",
        "get_gist",
        "summarize",
        "update",
        "invalidate",
        "discard",
        "maintenance_status",
        "consolidate",
        "intent_gate",
        "retrieve",
        "compose_context",
        "associations",
        "persona_get",
        "persona_put_stable",
        "persona_observe_preference",
        "task_upsert",
        "task_get",
        "task_history",
        "failure_record",
        "failures_for_task",
        "memtree_query",
        "memtree_get",
        "temporal_history",
        "triple_add",
        "triple_query",
        "triple_end",
        "belief_observe",
        "belief_history",
        "graph_link",
        "graph_query",
        "inspect",
        "correct",
        "audit_trail",
        "export",
        "erase_request",
    ];
    for name in core {
        entries.push(tool_entry(name, "core"));
    }

    let additive: &[&str] = &[
        "batch",
        "scratchpad_write",
        "scratchpad_read",
        "scratchpad_clear",
        "canonical_put",
        "canonical_get",
        "shared_store",
        "shared_retrieve",
        "shared_discard",
        "validate",
        "hygiene_audit",
        "hygiene_clean",
        "hygiene_log_list",
        "stats",
        "import",
        "import_provider",
        "diagnose",
        "verify",
        "doctor",
        "repair",
        "reindex",
    ];
    for name in additive {
        entries.push(tool_entry(name, "additive"));
    }

    if !omit_sync {
        for name in [
            "sync_serve",
            "sync_push",
            "sync_pull",
            "sync_status",
            "sync_key_status",
            "sync_ack_skip",
        ] {
            entries.push(tool_entry(name, "additive_sync"));
        }
    }

    for name in [
        "config_get",
        "config_set",
        "config_profiles",
        "config_profile_apply",
        "ranking_env_get",
        "ranking_env_set",
        "retention_profile_get",
        "retention_profile_set",
    ] {
        entries.push(tool_entry(name, "config_ranking"));
    }

    entries
}

/// One inventory entry whose `implemented` flag is read from the tool names the
/// build binds over MCP, so a tool the MCP layer dispatches is never reported as
/// a stub, and a name that is not bound is still refused as one.
fn tool_entry(name: &'static str, group: &'static str) -> ToolInventoryEntry {
    let implemented = clio_types::mcp_tools::is_bound(name);
    ToolInventoryEntry {
        name,
        group,
        implemented,
        status: if implemented {
            ToolImplStatus::Implemented
        } else {
            ToolImplStatus::Stub
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coding_local_omits_sync_and_denies_fr10() {
        let p = Profile::coding_local();
        assert!(p.omit_sync);
        assert!(!p.fr10_satisfied);
        assert!(p.mcp_transports.contains(&"streamable_http".into()));
        assert!(!p.mcp_transports.iter().any(|t| t.contains("sse")));
    }

    #[test]
    fn inventory_reports_the_bound_tool_surface() {
        let inv = coding_tool_inventory(true);
        for entry in &inv {
            assert_eq!(
                entry.implemented,
                clio_types::mcp_tools::is_bound(entry.name),
                "{} must be reported as the build binds it",
                entry.name
            );
        }
        let store = inv.iter().find(|e| e.name == "store").expect("store");
        assert!(
            store.implemented && store.status == ToolImplStatus::Implemented,
            "store dispatches over MCP, so it is implemented"
        );
        let cfg = inv.iter().find(|e| e.name == "config_get").expect("cfg");
        assert!(cfg.implemented);
        assert!(!clio_types::mcp_tools::is_bound("not_a_real_tool"));
        assert!(!inv.iter().any(|e| e.name.starts_with("sync_")));
    }

    #[test]
    fn coding_ci_and_sync_inventory() {
        let ci = Profile::coding_ci();
        assert_eq!(ci.name, "coding_ci");
        assert_eq!(ci.log_level, "warn");
        assert!(lookup_profile("coding_local").is_some());
        assert!(lookup_profile("missing").is_none());
        let with_sync = coding_tool_inventory(false);
        assert!(with_sync.iter().any(|e| e.name == "sync_status"));
        assert_eq!(builtin_profiles().len(), 2);
    }
}
