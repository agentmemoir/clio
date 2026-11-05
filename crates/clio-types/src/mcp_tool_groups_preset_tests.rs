// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for pinning each preset's tool list to the approved
//! catalog names, written out here instead of being rebuilt from the same rows.
//!
//! ## Owns
//! - The minimal preset's exact tool list, in catalog order
//! - The `all` preset's exact tool list, every approved row, in catalog order
//!
//! ## Does not own
//! - Group closure, name parsing, or custom selection (`mcp_tool_groups_tests`)
//! - The approved group rows and preset fixtures (`mcp_tool_groups_contract_tests`)
//!
//! ## Boundary
//!
//! Changes to approved preset membership must update the literal lists here.
//! Changes to query behavior should not require changes here.
//!
//! Keep this module focused on the preset-to-catalog pin.

use super::{DEFAULT_PRESET, ToolPreset};

/// The minimal preset: the retrieval and graph rows, in catalog order.
const MINIMAL_TOOLS: [&str; 6] = [
    "intent_gate",
    "retrieve",
    "compose_context",
    "associations",
    "graph_link",
    "graph_query",
];

/// The `all` preset: every approved row, in catalog order.
const ALL_TOOLS: [&str; 86] = [
    "intent_gate",
    "retrieve",
    "compose_context",
    "associations",
    "admit_preview",
    "admit_preview_batch",
    "store",
    "update",
    "invalidate",
    "discard",
    "correct",
    "batch",
    "validate",
    "canonical_put",
    "shared_store",
    "shared_discard",
    "scratchpad_write",
    "scratchpad_clear",
    "get",
    "get_snapshot",
    "get_gist",
    "inspect",
    "audit_trail",
    "summarize",
    "consolidate",
    "maintenance_status",
    "canonical_get",
    "shared_retrieve",
    "scratchpad_read",
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
    "hygiene_audit",
    "hygiene_clean",
    "hygiene_log_list",
    "export",
    "import",
    "import_provider",
    "review_export",
    "diagnose",
    "verify",
    "doctor",
    "repair",
    "reindex",
    "stats",
    "retention_profile_get",
    "retention_profile_set",
    "config_get",
    "config_set",
    "config_profiles",
    "config_profile_apply",
    "ranking_env_get",
    "ranking_env_set",
    "sync_push",
    "sync_pull",
    "sync_serve",
    "sync_ack_skip",
    "sync_status",
    "sync_key_status",
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
    "erase_request",
];

#[test]
fn the_minimal_preset_names_exactly_its_approved_rows() {
    assert_eq!(ToolPreset::Minimal.tools(), MINIMAL_TOOLS.to_vec());
    assert_eq!(DEFAULT_PRESET, ToolPreset::Minimal);
}

#[test]
fn the_all_preset_names_every_approved_row() {
    assert_eq!(ToolPreset::All.tools(), ALL_TOOLS.to_vec());
}

#[test]
fn a_wider_preset_is_a_superset_of_the_minimal_one() {
    let all = ToolPreset::All.tools();
    for name in MINIMAL_TOOLS {
        assert!(all.contains(&name), "`all` dropped the minimal tool `{name}`");
    }
    assert!(!MINIMAL_TOOLS.contains(&"store"));
}
