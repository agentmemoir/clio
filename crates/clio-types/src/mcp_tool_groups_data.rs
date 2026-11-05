// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the approved tool-group data: the row shape and the catalog that assigns every bound MCP tool to one business group.
//!
//! ## Owns
//! - `GroupSpec`, the row shape shared with the catalog
//! - `GROUP_TABLE`, the approved membership and guidance sentences
//!
//! ## Does not own
//! - Group queries, presets, and closures (`mcp_tool_groups`)
//! - The bound tool-name set itself (`mcp_tools`)
//!
//! ## Boundary
//!
//! Approved membership and guidance changes belong here; query logic does not.
//!
//! Keep this module focused on the approved tool-group data.

use crate::mcp_tool_groups::ToolGroup;

/// One tool-group catalog row: the group, its guidance sentence, and its tools.
#[derive(Debug, Clone, Copy)]
pub struct GroupSpec {
    /// The group this row describes.
    pub group: ToolGroup,
    /// One sentence telling an agent when to use the group.
    pub when_to_use: &'static str,
    /// Every bound tool that belongs to the group.
    pub tools: &'static [&'static str],
}

/// The tool-group catalog in canonical order. This is the only membership store.
pub const GROUP_TABLE: &[GroupSpec] = &[
    GroupSpec {
        group: ToolGroup::Retrieval,
        when_to_use: "Find memory or build grounded context.",
        tools: &["intent_gate", "retrieve", "compose_context", "associations"],
    },
    GroupSpec {
        group: ToolGroup::ItemWrite,
        when_to_use: "Create, correct or remove memory items.",
        tools: &[
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
        ],
    },
    GroupSpec {
        group: ToolGroup::ItemRead,
        when_to_use: "Read, inspect or summarize stored items.",
        tools: &[
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
        ],
    },
    GroupSpec {
        group: ToolGroup::Persona,
        when_to_use: "Read or record stable preferences.",
        tools: &[
            "persona_get",
            "persona_put_stable",
            "persona_observe_preference",
        ],
    },
    GroupSpec {
        group: ToolGroup::Task,
        when_to_use: "Record task state and failure history.",
        tools: &[
            "task_upsert",
            "task_get",
            "task_history",
            "failure_record",
            "failures_for_task",
        ],
    },
    GroupSpec {
        group: ToolGroup::Memtree,
        when_to_use: "Read hierarchical or temporal memory.",
        tools: &["memtree_query", "memtree_get", "temporal_history"],
    },
    GroupSpec {
        group: ToolGroup::Triple,
        when_to_use: "Record or query factual triples.",
        tools: &["triple_add", "triple_query", "triple_end"],
    },
    GroupSpec {
        group: ToolGroup::Belief,
        when_to_use: "Record or inspect beliefs.",
        tools: &["belief_observe", "belief_history"],
    },
    GroupSpec {
        group: ToolGroup::Graph,
        when_to_use: "Read or add memory relationships.",
        tools: &["graph_link", "graph_query"],
    },
    GroupSpec {
        group: ToolGroup::Hygiene,
        when_to_use: "Inspect or clean memory hygiene.",
        tools: &["hygiene_audit", "hygiene_clean", "hygiene_log_list"],
    },
    GroupSpec {
        group: ToolGroup::Portability,
        when_to_use: "Move or review memory exports and imports.",
        tools: &["export", "import", "import_provider", "review_export"],
    },
    GroupSpec {
        group: ToolGroup::Ops,
        when_to_use: "Diagnose, verify or repair operations.",
        tools: &["diagnose", "verify", "doctor", "repair", "reindex", "stats"],
    },
    GroupSpec {
        group: ToolGroup::Retention,
        when_to_use: "Read or set retention policy.",
        tools: &["retention_profile_get", "retention_profile_set"],
    },
    GroupSpec {
        group: ToolGroup::Config,
        when_to_use: "Read or change configuration.",
        tools: &[
            "config_get",
            "config_set",
            "config_profiles",
            "config_profile_apply",
        ],
    },
    GroupSpec {
        group: ToolGroup::Ranking,
        when_to_use: "Read or change ranking settings.",
        tools: &["ranking_env_get", "ranking_env_set"],
    },
    GroupSpec {
        group: ToolGroup::Sync,
        when_to_use: "Exchange memory between peers.",
        tools: &[
            "sync_push",
            "sync_pull",
            "sync_serve",
            "sync_ack_skip",
            "sync_status",
            "sync_key_status",
        ],
    },
    GroupSpec {
        group: ToolGroup::Compliance,
        when_to_use: "Track subject requests and erasure.",
        tools: &[
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
        ],
    },
];
