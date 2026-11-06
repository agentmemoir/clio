// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module owns the query and derivation layer over the approved tool-group data: names, presets, closures, and custom selections.
//!
//! ## Owns
//! - `ToolGroup`, `ToolPreset`, and `DEFAULT_PRESET`
//! - `closure`, `custom`, and `tools_of`, which turn a selection into groups
//!   and tools
//! - The public re-export of `GroupSpec` and `GROUP_TABLE`
//!
//! ## Does not own
//! - The approved membership and guidance data (`mcp_tool_groups_data`)
//! - Dispatching, listing, or advertising a tool (`clio-mcp`)
//! - The bound tool-name set itself (`mcp_tools`)
//!
//! ## Boundary
//!
//! Group membership data lives in `mcp_tool_groups_data`; query and preset logic lives here.
//!
//! Keep this module focused on the tool-group derivations.

pub use crate::mcp_tool_groups_data::{GROUP_TABLE, GroupSpec};

/// A business grouping of the MCP tools bound in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolGroup {
    /// Search and context assembly.
    Retrieval,
    /// Create, correct, or remove memory items.
    ItemWrite,
    /// Read, inspect, or summarize stored items.
    ItemRead,
    /// Read or record stable preferences.
    Persona,
    /// Record task state and failure history.
    Task,
    /// Read hierarchical or temporal memory.
    Memtree,
    /// Record or query factual triples.
    Triple,
    /// Record or inspect beliefs.
    Belief,
    /// Read or add memory relationships.
    Graph,
    /// Inspect or clean memory hygiene.
    Hygiene,
    /// Move or review memory exports and imports.
    Portability,
    /// Diagnose, verify, or repair operations.
    Ops,
    /// Read or set retention policy.
    Retention,
    /// Read or change configuration.
    Config,
    /// Read or change ranking settings.
    Ranking,
    /// Exchange memory between peers.
    Sync,
    /// Track subject requests and erasure.
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
            .expect("every tool group is listed in ToolGroup::ALL")
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
    /// The default minimal starting set.
    Minimal,
    /// A coding-focused set.
    Coding,
    /// An operations-focused set.
    Ops,
    /// Every group.
    All,
}

/// The preset used when no other preset is selected.
pub const DEFAULT_PRESET: ToolPreset = ToolPreset::Minimal;

impl ToolPreset {
    /// Every preset, in declaration order.
    pub const ALL: [Self; 4] = [Self::Minimal, Self::Coding, Self::Ops, Self::All];

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
        Self::ALL.into_iter().find(|preset| preset.as_str() == name)
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

    /// Every tool the preset's resolved groups hold, in catalog order.
    #[must_use]
    pub fn tools(self) -> Vec<&'static str> {
        tools_of(self.groups())
    }
}

/// Every tool a selection of `groups` holds, in catalog order.
///
/// This is the one groups-to-tools derivation: a preset and a caller-supplied
/// selection both resolve through [`closure`] here, so the two can never
/// advertise different tools for the same groups.
#[must_use]
pub fn tools_of(groups: &[ToolGroup]) -> Vec<&'static str> {
    closure(groups)
        .into_iter()
        .flat_map(|group| group.spec().tools.iter().copied())
        .collect()
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

#[cfg(test)]
#[path = "mcp_tool_groups_contract_tests.rs"]
mod contract_tests;

#[cfg(test)]
#[path = "mcp_tool_groups_preset_tests.rs"]
mod preset_tests;
