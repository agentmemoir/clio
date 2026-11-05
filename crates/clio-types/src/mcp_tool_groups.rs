// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module owns the catalog that maps every bound MCP tool to one business
//! tool group, states when to use each group, and derives presets and closures.
//!
//! ## Owns
//! - `ToolGroup`, `GroupSpec`, and `GROUP_TABLE`, the single membership store
//! - `ToolPreset`, `DEFAULT_PRESET`, and the declared preset group sets
//! - `closure` and `custom`, which add transitive group requirements
//!
//! ## Does not own
//! - Dispatching, listing, or advertising a tool (`clio-mcp`)
//! - The bound tool-name set itself (`mcp_tools`)
//!
//! ## Boundary
//!
//! Group membership, guidance, and preset changes belong here; tool behaviour
//! and schema do not.
//!
//! Keep this module focused on the tool-group catalog and its derivations.

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

/// A business grouping of the MCP tools bound in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolGroup {
    Retrieval,
    ItemWrite,
    ItemRead,
    Persona,
    Task,
    Memtree,
    Triple,
    Belief,
    Graph,
    Hygiene,
    Portability,
    Ops,
    Retention,
    Config,
    Ranking,
    Sync,
    Compliance,
}

impl ToolGroup {
    /// Every group, in catalog order.
    pub const ALL: [Self; 17] = [
        Self::Retrieval,
        Self::ItemWrite,
        Self::ItemRead,
        Self::Persona,
        Self::Task,
        Self::Memtree,
        Self::Triple,
        Self::Belief,
        Self::Graph,
        Self::Hygiene,
        Self::Portability,
        Self::Ops,
        Self::Retention,
        Self::Config,
        Self::Ranking,
        Self::Sync,
        Self::Compliance,
    ];

    /// The stable `snake_case` name of the group.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retrieval => "retrieval",
            Self::ItemWrite => "item-write",
            Self::ItemRead => "item-read",
            Self::Persona => "persona",
            Self::Task => "task",
            Self::Memtree => "memtree",
            Self::Triple => "triple",
            Self::Belief => "belief",
            Self::Graph => "graph",
            Self::Hygiene => "hygiene",
            Self::Portability => "portability",
            Self::Ops => "ops",
            Self::Retention => "retention",
            Self::Config => "config",
            Self::Ranking => "ranking",
            Self::Sync => "sync",
            Self::Compliance => "compliance",
        }
    }

    /// Parse a stable group name.
    ///
    /// The name is a stable wire word, so lookup returns `None` instead of a
    /// parse error and never allocates.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|group| group.as_str() == name)
    }

    /// The catalog row for this group.
    ///
    /// # Panics
    ///
    /// Panics if the group has no row in `GROUP_TABLE`, which cannot happen for
    /// a value of this enum.
    #[must_use]
    pub fn spec(self) -> &'static GroupSpec {
        GROUP_TABLE
            .iter()
            .find(|row| row.group == self)
            .expect("every tool group has a catalog row")
    }

    /// The groups this group needs before it is useful.
    #[must_use]
    pub const fn requires(self) -> &'static [Self] {
        match self {
            Self::ItemWrite | Self::Hygiene | Self::Ops | Self::Retention => &[Self::ItemRead],
            Self::Portability | Self::Sync | Self::Compliance => &[Self::ItemRead, Self::ItemWrite],
            _ => &[],
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|group| *group == self)
            .unwrap_or(0)
    }
}

/// The group that owns `tool`, or `None` when no group lists it.
#[must_use]
pub fn group_of(tool: &str) -> Option<ToolGroup> {
    GROUP_TABLE
        .iter()
        .find(|row| row.tools.contains(&tool))
        .map(|row| row.group)
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

const MINIMAL_GROUPS: [ToolGroup; 2] = [ToolGroup::Retrieval, ToolGroup::Graph];
const CODING_GROUPS: [ToolGroup; 3] = [
    ToolGroup::Retrieval,
    ToolGroup::ItemRead,
    ToolGroup::Persona,
];
const OPS_GROUPS: [ToolGroup; 4] = [
    ToolGroup::ItemRead,
    ToolGroup::Ops,
    ToolGroup::Hygiene,
    ToolGroup::Retention,
];

/// A named starting set of tool groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolPreset {
    Minimal,
    Coding,
    Ops,
    All,
}

/// The preset used when no other preset is selected.
pub const DEFAULT_PRESET: ToolPreset = ToolPreset::Minimal;

impl ToolPreset {
    /// The stable `snake_case` name of the preset.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Coding => "coding",
            Self::Ops => "ops",
            Self::All => "all",
        }
    }

    /// Parse a stable preset name.
    ///
    /// The name is a stable wire word, so lookup returns `None` instead of a
    /// parse error and never allocates.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(name: &str) -> Option<Self> {
        [Self::Minimal, Self::Coding, Self::Ops, Self::All]
            .into_iter()
            .find(|preset| preset.as_str() == name)
    }

    /// The groups a preset declares, without dependency closure.
    #[must_use]
    pub fn groups(self) -> &'static [ToolGroup] {
        match self {
            Self::Minimal => &MINIMAL_GROUPS,
            Self::Coding => &CODING_GROUPS,
            Self::Ops => &OPS_GROUPS,
            Self::All => &ToolGroup::ALL,
        }
    }

    /// The preset's groups with their dependency closure, in canonical order.
    #[must_use]
    pub fn resolve(self) -> Vec<ToolGroup> {
        closure(self.groups())
    }
}

/// Add every transitive requirement of `groups`, in canonical order without
/// duplicates.
#[must_use]
pub fn closure(groups: &[ToolGroup]) -> Vec<ToolGroup> {
    let mut included = [false; ToolGroup::ALL.len()];
    for group in groups {
        included[group.index()] = true;
    }
    loop {
        let mut changed = false;
        for (index, group) in ToolGroup::ALL.iter().enumerate() {
            if !included[index] {
                continue;
            }
            for requirement in group.requires() {
                let requirement_index = requirement.index();
                if !included[requirement_index] {
                    included[requirement_index] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    ToolGroup::ALL
        .iter()
        .zip(included)
        .filter_map(|(group, present)| present.then_some(*group))
        .collect()
}

/// Resolve a custom selection against the minimal preset.
///
/// # Errors
///
/// Returns `ToolGroupError::UnknownGroup` naming the first selection that is not
/// a group, before any partial result is built.
pub fn custom(selected: &[&str]) -> Result<Vec<ToolGroup>, ToolGroupError> {
    let mut resolved = Vec::with_capacity(selected.len());
    for name in selected {
        let group = ToolGroup::from_str(name).ok_or_else(|| ToolGroupError::UnknownGroup {
            name: (*name).to_string(),
        })?;
        resolved.push(group);
    }
    let mut union = ToolPreset::Minimal.groups().to_vec();
    union.extend(resolved);
    Ok(closure(&union))
}

/// A failure while resolving a custom group selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolGroupError {
    /// The selection named something that is not a tool group.
    UnknownGroup {
        /// The name that was given.
        name: String,
    },
}

impl std::fmt::Display for ToolGroupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownGroup { name } => write!(formatter, "unknown tool group `{name}`"),
        }
    }
}

impl std::error::Error for ToolGroupError {}

#[cfg(test)]
#[path = "mcp_tool_groups_tests.rs"]
mod tests;
