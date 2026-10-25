// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the one source of truth for the tool names bound over MCP in
//! this build, shared by the MCP layer that advertises them and the config layer
//! that reports a deployment's capability.
//!
//! ## Owns
//! - The bound write, read and combined tool-name lists, and `is_bound`
//!
//! ## Does not own
//! - Dispatching a tool or publishing its schema (`clio-mcp`, which routes each
//!   bound name through its own dispatch chain)
//! - The deployment profile that declares a subset (`clio-config`)
//!
//! ## Boundary
//!
//! Adding or removing a bound tool should normally require changes here.
//! Tool behaviour should not require changes here.
//!
//! Keep this module focused on the bound tool-name set and nothing else.

/// Tool names bound over MCP in this build (the write/mutate set).
pub const BOUND_WRITE_TOOLS: &[&str] = &[
    "admit_preview",
    "store",
    "update",
    "invalidate",
    "discard",
    "correct",
    "consolidate",
    "persona_put_stable",
    "persona_observe_preference",
    "task_upsert",
    "failure_record",
    "triple_add",
    "triple_end",
    "belief_observe",
    "graph_link",
    "erase_request",
    "batch",
    "scratchpad_write",
    "scratchpad_read",
    "scratchpad_clear",
    "canonical_put",
    "shared_store",
    "shared_discard",
    "validate",
    "hygiene_clean",
    "export",
    "import",
    "import_provider",
    "review_export",
    "repair",
    "reindex",
    "sync_push",
    "sync_pull",
    "sync_serve",
    "sync_ack_skip",
    "retention_profile_set",
    "config_set",
    "config_profile_apply",
    "ranking_env_set",
    "subject_request_start",
    "subject_request_status",
    "subject_request_retry",
    "subject_request_copy",
    "subject_request_exception",
    "subject_request_events",
    "subject_request_path_complete",
    "subject_request_close",
    "subject_request_peer_status",
    "subject_request_peer_retry",
    "subject_request_peer_disposition",
];

/// Read/retrieve/compose tool names bound over MCP in this build (read rows).
pub const BOUND_READ_TOOLS: &[&str] = &[
    "intent_gate",
    "retrieve",
    "compose_context",
    "associations",
    "get",
    "get_snapshot",
    "get_gist",
    "maintenance_status",
    "persona_get",
    "task_get",
    "task_history",
    "failures_for_task",
    "memtree_query",
    "memtree_get",
    "temporal_history",
    "triple_query",
    "belief_history",
    "graph_query",
    "inspect",
    "audit_trail",
    "canonical_get",
    "shared_retrieve",
    "stats",
    "hygiene_audit",
    "hygiene_log_list",
    "diagnose",
    "verify",
    "doctor",
    "sync_status",
    "sync_key_status",
    "admit_preview_batch",
    "retention_profile_get",
    "summarize",
    "config_get",
    "config_profiles",
    "ranking_env_get",
];

/// Every tool name bound over MCP in this build (write set + read surface).
#[must_use]
pub fn bound_tools() -> Vec<&'static str> {
    let mut all = BOUND_WRITE_TOOLS.to_vec();
    all.extend_from_slice(BOUND_READ_TOOLS);
    all
}

/// Whether `name` is one of the tools bound over MCP in this build.
#[must_use]
pub fn is_bound(name: &str) -> bool {
    BOUND_WRITE_TOOLS.contains(&name) || BOUND_READ_TOOLS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn completion_is_a_unique_bound_write_tool() {
        let tools = bound_tools();
        assert_eq!(tools.len(), tools.iter().collect::<BTreeSet<_>>().len());
        assert!(BOUND_WRITE_TOOLS.contains(&"subject_request_path_complete"));
        assert!(!BOUND_READ_TOOLS.contains(&"subject_request_path_complete"));
        for tool in tools {
            assert!(is_bound(tool), "published tool {tool} must be bound");
        }
        assert!(!is_bound("subject_request_path_uncomplete"));
    }
}
