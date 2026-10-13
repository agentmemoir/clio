// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the commands headed by
//! `belief`, `graph`, and the single `associations` command: `belief history`,
//! `belief observe`, `graph query`, `graph link`, and `associations`.
//!
//! It is the sibling the `cli_command_registry_graph_records` module declares
//! and re-exports, so the tranche's `GRAPH_RECORDS` list can name these records
//! while each half of the tranche stays within the size cap.
//!
//! ## Owns
//! - The five `CommandSpec` records and the positionals and flags only they use
//! - The description, value kind, and default of each documented flag
//!
//! ## Does not own
//! - The triple records and the tranche list (see
//!   `cli_command_registry_graph_records`)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The command semantics (see the belief, graph, and write surfaces)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags or summary text should normally
//! require changes here. Changes to the command implementations should not.
//!
//! Keep this module focused on the belief, graph, and associations records.

use super::{
    AS_OF, CONTEXT, DRY_RUN, EXPLAIN, SUBJECT_ID, UNPRINTED_GLOBALS, UNPRINTED_GLOBALS_OUTSIDE_BANK,
};
use crate::cli_command_registry::{GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, Section};
use clio_types::SourceType;

/// The required positional of `belief history`.
const BELIEF_ID: Positional = Positional {
    label: "BELIEF_ID_OR_PROPOSITION",
    required: true,
    description: "Belief id or proposition text whose confidence history is printed.",
};

/// The required positional of `belief observe`.
const PROP: Positional = Positional {
    label: "PROP",
    required: true,
    description: "Proposition text observed; an existing belief appends, new text creates one.",
};

/// The required positional of `graph query`.
const SEED_ID: Positional = Positional {
    label: "SEED_ID",
    required: true,
    description: "Item id the neighbour walk starts from.",
};

/// The first required positional of `graph link`.
const LINK_A: Positional = Positional {
    label: "A",
    required: true,
    description: "Source item id of the explicit edge.",
};

/// The second required positional of `graph link`.
const LINK_B: Positional = Positional {
    label: "B",
    required: true,
    description: "Target item id of the explicit edge.",
};

/// The required positional of `associations`.
const ITEM_ID: Positional = Positional {
    label: "ITEM_ID",
    required: true,
    description: "Item id whose co-activation and explicit edges are listed.",
};

/// The `--min-weight W` of `graph query` and `associations`.
const MIN_WEIGHT: Flag = Flag::float("min-weight", "W", "Sets the minimum edge weight.");

/// `clio belief history`: one belief's confidence history, over the
/// `belief_history` tool.
pub(crate) const BELIEF_HISTORY: CommandSpec = CommandSpec {
    name: "belief history",
    aliases: &[],
    summary: "one belief's confidence history",
    notes: &[],
    positionals: &[BELIEF_ID],
    documented: &[EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio belief history \"the deploy is blocked\" --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio belief history \"the deploy is blocked\" --explain",
            comment: Some("by proposition"),
        },
        Example {
            command: "clio belief history \"the deploy is blocked\" -o json",
            comment: None,
        },
    ],
    tool: Some("belief_history"),
    section: Section::Read,
};

/// `clio belief observe`: record evidence and a new confidence, over the
/// `belief_observe` tool.
pub(crate) const BELIEF_OBSERVE: CommandSpec = CommandSpec {
    name: "belief observe",
    aliases: &[],
    summary: "record evidence and a new confidence",
    notes: &[],
    positionals: &[PROP],
    documented: &[
        Flag {
            required: true,
            ..Flag::float(
                "confidence",
                "C",
                "`belief_observe_call` requires the `0.0..=1.0` confidence.",
            )
        },
        Flag {
            required: true,
            ..Flag::text(
                "evidence",
                "E",
                "`belief_observe_call` requires the `evidence_ref`.",
            )
        },
        Flag {
            required: true,
            ..Flag::choice(
                "source",
                "S",
                "`belief_observe_call` requires a `source_type`: `user_stated|agent_inferred|third_party`.",
                SourceType::WIRE,
            )
        },
        AS_OF,
        SUBJECT_ID,
        CONTEXT,
        DRY_RUN,
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio belief observe \"the deploy is blocked\" --confidence 0.8 --evidence run-42 --source user_stated --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio belief observe \"the deploy is blocked\" --confidence 0.8 --evidence run-42 --source agent_inferred --dry-run",
            comment: Some("preview"),
        },
        Example {
            command: "clio belief observe \"the deploy is blocked\" --confidence 0.8 --evidence run-42 --source third_party --as-of 2026-01-01",
            comment: None,
        },
    ],
    tool: Some("belief_observe"),
    section: Section::Write,
};

/// `clio graph query`: neighbours of one item, by edge type, over the
/// `graph_query` tool.
pub(crate) const GRAPH_QUERY: CommandSpec = CommandSpec {
    name: "graph query",
    aliases: &[],
    summary: "neighbours of one item, by edge type",
    notes: &[],
    positionals: &[SEED_ID],
    documented: &[
        Flag::uint("max-hops", "N", "`graph_query_call` sets the hop count."),
        Flag::choice(
            "edge-type",
            "coactivation|explicit",
            "`graph_query_call` validates `coactivation` or `explicit`.",
            &["coactivation", "explicit"],
        ),
        MIN_WEIGHT,
        EXPLAIN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio graph query itm-sample-1 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio graph query itm-sample-1 --max-hops 2 --edge-type explicit",
            comment: None,
        },
        Example {
            command: "clio graph query itm-sample-1 --min-weight 0.5 -o json",
            comment: None,
        },
    ],
    tool: Some("graph_query"),
    section: Section::Read,
};

/// `clio graph link`: add an explicit edge between two items, over the
/// `graph_link` tool.
pub(crate) const GRAPH_LINK: CommandSpec = CommandSpec {
    name: "graph link",
    aliases: &[],
    summary: "add an explicit edge between two items",
    notes: &[],
    positionals: &[LINK_A, LINK_B],
    documented: &[
        Flag {
            required: true,
            ..Flag::text(
                "relationship",
                "R",
                "`graph_link_call` requires the association label.",
            )
        },
        Flag {
            default: Some("1.0"),
            ..Flag::float(
                "weight",
                "W",
                "`graph_link_call` sets the edge weight, and the tool defaults it to `1.0`.",
            )
        },
        DRY_RUN,
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio graph link itm-sample-1 itm-sample-2 --relationship related --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio graph link itm-sample-1 itm-sample-2 --relationship related --weight 0.8",
            comment: None,
        },
        Example {
            command: "clio graph link itm-sample-1 itm-sample-2 --relationship related --dry-run",
            comment: Some("preview"),
        },
    ],
    tool: Some("graph_link"),
    section: Section::Write,
};

/// `clio associations`: co-activation links for one item, over the
/// `associations` tool.
pub(crate) const ASSOCIATIONS: CommandSpec = CommandSpec {
    name: "associations",
    aliases: &[],
    summary: "co-activation links for one item",
    notes: &[],
    positionals: &[ITEM_ID],
    documented: &[MIN_WEIGHT, EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio associations itm-sample-1 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio associations itm-sample-1 --min-weight 0.3",
            comment: None,
        },
        Example {
            command: "clio associations itm-sample-1 --explain -o json",
            comment: None,
        },
    ],
    tool: Some("associations"),
    section: Section::Read,
};

#[cfg(test)]
#[path = "cli_command_registry_graph_records_belief_graph_tests.rs"]
mod tests;
