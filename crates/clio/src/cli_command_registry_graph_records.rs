// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the commands headed by
//! `triple`, `belief`, `graph`, and the single `associations` command:
//! `triple query`, `triple add`, `triple end`, `belief history`,
//! `belief observe`, `graph query`, `graph link`, and `associations`.
//!
//! A read head can hold a write subcommand: `triple add`, `triple end`,
//! `belief observe`, and `graph link` write, but they are classified under
//! their read head like every other record here.
//!
//! ## Owns
//! - The eight `CommandSpec` records and `GRAPH_RECORDS`, the tranche that
//!   lists them in head-group order
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry lookups and the global order of the rendered surfaces (see
//!   `cli_command_registry` and `cli_command_bindings`)
//! - The command semantics (see the graph, belief, and write surfaces)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags or summary text should normally
//! require changes here. Changes to the command implementations should not.
//!
//! Keep this module focused on the triple, belief, graph, and associations
//! records.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// The optional `SUBJECT` positional of `triple query`.
const SUBJECT: Positional = Positional {
    label: "SUBJECT",
    required: false,
    description: "",
};

/// The optional `PREDICATE` positional of `triple query`.
const PREDICATE: Positional = Positional {
    label: "PREDICATE",
    required: false,
    description: "",
};

/// The optional `OBJECT` positional of `triple query`.
const OBJECT: Positional = Positional {
    label: "OBJECT",
    required: false,
    description: "",
};

/// The required `S` positional of `triple add` and `triple end`.
const TRIPLE_S: Positional = Positional {
    label: "S",
    required: true,
    description: "",
};

/// The required `P` positional of `triple add` and `triple end`.
const TRIPLE_P: Positional = Positional {
    label: "P",
    required: true,
    description: "",
};

/// The required `O` positional of `triple add`.
const TRIPLE_O: Positional = Positional {
    label: "O",
    required: true,
    description: "",
};

/// The required `BELIEF_ID_OR_PROPOSITION` positional of `belief history`.
const BELIEF_ID_OR_PROPOSITION: Positional = Positional {
    label: "BELIEF_ID_OR_PROPOSITION",
    required: true,
    description: "",
};

/// The required `PROP` positional of `belief observe`.
const PROP: Positional = Positional {
    label: "PROP",
    required: true,
    description: "",
};

/// The required `SEED_ID` positional of `graph query`.
const SEED_ID: Positional = Positional {
    label: "SEED_ID",
    required: true,
    description: "",
};

/// The required `A` positional of `graph link`.
const GRAPH_A: Positional = Positional {
    label: "A",
    required: true,
    description: "",
};

/// The required `B` positional of `graph link`.
const GRAPH_B: Positional = Positional {
    label: "B",
    required: true,
    description: "",
};

/// The required `ITEM_ID` positional of `associations`.
const ITEM_ID: Positional = Positional {
    label: "ITEM_ID",
    required: true,
    description: "",
};

/// The `--explain` trace flag the read commands here document.
const EXPLAIN: Flag = Flag::boolean("explain");

/// The `--dry-run` preview flag the write commands here document.
const DRY_RUN: Flag = Flag::boolean("dry-run");

/// The required `--valid-until TS` flag of `triple end`.
const REQUIRED_VALID_UNTIL: Flag = Flag {
    required: true,
    ..Flag::value("valid-until", "TS")
};

/// The required `--confidence C` flag of `belief observe`.
const REQUIRED_CONFIDENCE: Flag = Flag {
    required: true,
    ..Flag::value("confidence", "C")
};

/// The required `--evidence E` flag of `belief observe`.
const REQUIRED_EVIDENCE: Flag = Flag {
    required: true,
    ..Flag::value("evidence", "E")
};

/// The required `--source S` flag of `belief observe`.
const REQUIRED_SOURCE: Flag = Flag {
    required: true,
    ..Flag::value("source", "S")
};

/// The required `--relationship R` flag of `graph link`.
const REQUIRED_RELATIONSHIP: Flag = Flag {
    required: true,
    ..Flag::value("relationship", "R")
};

/// The declared globals these commands accept without printing: every global
/// but `--output`, which each usage line documents.
const UNPRINTED_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The `--epistemic-kind`/`--source-type` pairing `triple add` prints under
/// its usage line.
const SOURCE_TYPE_PAIRING_NOTE: &str = "--epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact";

/// `triple query`'s usage line: its three positionals are nested optionals, a
/// shape the assembled form cannot print.
const TRIPLE_QUERY_USAGE: &str = "clio triple query [SUBJECT [PREDICATE [OBJECT]]] [--subject S] [--predicate P] [--object O] [--as-of TS] [--time-axis valid|transaction] [--explain] [--output json|text]";

/// `clio triple query`: match subject/predicate/object patterns, over the
/// `triple_query` tool.
pub(crate) const TRIPLE_QUERY: CommandSpec = CommandSpec {
    name: "triple query",
    aliases: &[],
    summary: "match subject/predicate/object patterns",
    notes: &[],
    positionals: &[SUBJECT, PREDICATE, OBJECT],
    documented: &[
        Flag::value("subject", "S"),
        Flag::value("predicate", "P"),
        Flag::value("object", "O"),
        Flag::value("as-of", "TS"),
        Flag::value("time-axis", "valid|transaction"),
        EXPLAIN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: Some(TRIPLE_QUERY_USAGE),
    usage_alternates: &[],
    examples: &[],
    tool: Some("triple_query"),
    section: Section::Read,
};

/// `clio triple add`: write one subject/predicate/object fact, over the
/// `triple_add` tool.
pub(crate) const TRIPLE_ADD: CommandSpec = CommandSpec {
    name: "triple add",
    aliases: &[],
    summary: "write one subject/predicate/object fact",
    notes: &[SOURCE_TYPE_PAIRING_NOTE],
    positionals: &[TRIPLE_S, TRIPLE_P, TRIPLE_O],
    documented: &[
        Flag::value("epistemic-kind", "fact|belief"),
        Flag::value("source-type", "T"),
        Flag::value("confidence", "N"),
        Flag::value("context", "TEXT"),
        Flag::value("valid-from", "TS"),
        Flag::value("valid-until", "TS"),
        Flag::value("supersede", "true|false"),
        Flag::value("subject-id", "ID"),
        Flag::value("update-rule", "discrete"),
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("triple_add"),
    section: Section::Write,
};

/// `clio triple end`: close an open fact without a replacement, over the
/// `triple_end` tool.
pub(crate) const TRIPLE_END: CommandSpec = CommandSpec {
    name: "triple end",
    aliases: &[],
    summary: "close an open fact without a replacement",
    notes: &[],
    positionals: &[TRIPLE_S, TRIPLE_P],
    documented: &[
        Flag::value("object", "O"),
        REQUIRED_VALID_UNTIL,
        Flag::value("update-rule", "discrete"),
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("triple_end"),
    section: Section::Write,
};

/// `clio belief history`: one belief's confidence history, over the
/// `belief_history` tool.
pub(crate) const BELIEF_HISTORY: CommandSpec = CommandSpec {
    name: "belief history",
    aliases: &[],
    summary: "one belief's confidence history",
    notes: &[],
    positionals: &[BELIEF_ID_OR_PROPOSITION],
    documented: &[EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
        REQUIRED_CONFIDENCE,
        REQUIRED_EVIDENCE,
        REQUIRED_SOURCE,
        Flag::value("as-of", "TS"),
        Flag::value("subject-id", "ID"),
        Flag::value("context", "TEXT"),
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
        Flag::value("max-hops", "N"),
        Flag::value("edge-type", "coactivation|explicit"),
        Flag::value("min-weight", "W"),
        EXPLAIN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
    positionals: &[GRAPH_A, GRAPH_B],
    documented: &[
        REQUIRED_RELATIONSHIP,
        Flag::value("weight", "W"),
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
    documented: &[Flag::value("min-weight", "W"), EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("associations"),
    section: Section::Read,
};

/// The eight records, in head-group order: `triple`, `belief`, `graph`, then
/// `associations`.
pub(crate) static GRAPH_RECORDS: &[CommandSpec] = &[
    TRIPLE_QUERY,
    TRIPLE_ADD,
    TRIPLE_END,
    BELIEF_HISTORY,
    BELIEF_OBSERVE,
    GRAPH_QUERY,
    GRAPH_LINK,
    ASSOCIATIONS,
];

#[cfg(test)]
#[path = "cli_command_registry_graph_records_tests.rs"]
mod tests;
