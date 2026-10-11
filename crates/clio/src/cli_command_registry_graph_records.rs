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
//! - The description, value kind, and default of each documented flag
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
use clio_types::SourceType;

/// The optional `SUBJECT` positional of `triple query`.
const QUERY_SUBJECT: Positional = Positional {
    label: "SUBJECT",
    required: false,
    description: "",
};

/// The optional `PREDICATE` positional of `triple query`.
const QUERY_PREDICATE: Positional = Positional {
    label: "PREDICATE",
    required: false,
    description: "",
};

/// The optional `OBJECT` positional of `triple query`.
const QUERY_OBJECT: Positional = Positional {
    label: "OBJECT",
    required: false,
    description: "",
};

/// The required `S` positional of `triple add` and `triple end`.
const FACT_SUBJECT: Positional = Positional {
    label: "S",
    required: true,
    description: "",
};

/// The required `P` positional of `triple add` and `triple end`.
const FACT_PREDICATE: Positional = Positional {
    label: "P",
    required: true,
    description: "",
};

/// The required `O` positional of `triple add`.
const FACT_OBJECT: Positional = Positional {
    label: "O",
    required: true,
    description: "",
};

/// The required positional of `belief history`.
const BELIEF_ID: Positional = Positional {
    label: "BELIEF_ID_OR_PROPOSITION",
    required: true,
    description: "",
};

/// The required positional of `belief observe`.
const PROP: Positional = Positional {
    label: "PROP",
    required: true,
    description: "",
};

/// The required positional of `graph query`.
const SEED_ID: Positional = Positional {
    label: "SEED_ID",
    required: true,
    description: "",
};

/// The first required positional of `graph link`.
const LINK_A: Positional = Positional {
    label: "A",
    required: true,
    description: "",
};

/// The second required positional of `graph link`.
const LINK_B: Positional = Positional {
    label: "B",
    required: true,
    description: "",
};

/// The required positional of `associations`.
const ITEM_ID: Positional = Positional {
    label: "ITEM_ID",
    required: true,
    description: "",
};

/// The `--as-of TS` timestamp of `triple query` and `belief observe`.
const AS_OF: Flag = Flag::text("as-of", "TS", "Sets the reference `as_of` timestamp.");

/// The `--explain` trace flag the read commands here document.
const EXPLAIN: Flag = Flag::toggle("explain", "Asks `read_args` for the trace detail.");

/// The `--dry-run` preview flag the write commands here document.
const DRY_RUN: Flag = Flag::toggle("dry-run", "Previews with no durable change.");

/// The `--context TEXT` of `triple add` and `belief observe`.
const CONTEXT: Flag = Flag::text("context", "TEXT", "`context_arg` bounds it to 4096 bytes.");

/// The `--subject-id ID` of `triple add` and `belief observe`.
const SUBJECT_ID: Flag = Flag::text("subject-id", "ID", "Sets `subject_id` for DEK selection.");

/// The `--update-rule discrete` of `triple add` and `triple end`.
const UPDATE_RULE: Flag = Flag::choice(
    "update-rule",
    "discrete",
    "`cli_write_graph` forwards `update_rule`; this verb accepts only `discrete`.",
    &["discrete"],
);

/// The `--min-weight W` of `graph query` and `associations`.
const MIN_WEIGHT: Flag = Flag::float("min-weight", "W", "Sets the minimum edge weight.");

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
    positionals: &[QUERY_SUBJECT, QUERY_PREDICATE, QUERY_OBJECT],
    documented: &[
        Flag::text("subject", "S", "`triple_query_call` sets the pattern."),
        Flag::text("predicate", "P", "`triple_query_call` sets the pattern."),
        Flag::text("object", "O", "`triple_query_call` sets the pattern."),
        AS_OF,
        Flag {
            default: Some("valid"),
            ..Flag::choice(
                "time-axis",
                "valid|transaction",
                "`cli_read_belief` validates `valid` or `transaction`, and the tool defaults it to `valid`.",
                &["valid", "transaction"],
            )
        },
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
    positionals: &[FACT_SUBJECT, FACT_PREDICATE, FACT_OBJECT],
    documented: &[
        Flag {
            default: Some("fact"),
            ..Flag::choice(
                "epistemic-kind",
                "fact|belief",
                "`triple_add_call` defaults it to `fact`; accepts `fact` or `belief`.",
                &["fact", "belief"],
            )
        },
        Flag::choice(
            "source-type",
            "T",
            "`source_type_arg` requires it with `belief`: `user_stated|agent_inferred|third_party`.",
            SourceType::WIRE,
        ),
        Flag::float(
            "confidence",
            "N",
            "`triple_add_call` parses a unit confidence.",
        ),
        CONTEXT,
        Flag::text("valid-from", "TS", "`triple_add_call` sets `valid_from`."),
        Flag::text("valid-until", "TS", "`triple_add_call` sets `valid_until`."),
        Flag {
            default: Some("true"),
            ..Flag::choice(
                "supersede",
                "true|false",
                "`triple_add_call` accepts `true` or `false`, and defaults it to `true`.",
                &["true", "false"],
            )
        },
        SUBJECT_ID,
        UPDATE_RULE,
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
    positionals: &[FACT_SUBJECT, FACT_PREDICATE],
    documented: &[
        Flag::text("object", "O", "`triple_end_call` matches by object."),
        Flag {
            required: true,
            ..Flag::text(
                "valid-until",
                "TS",
                "`triple_end_call` requires the expiration `valid_until`.",
            )
        },
        UPDATE_RULE,
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
    positionals: &[BELIEF_ID],
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
    documented: &[MIN_WEIGHT, EXPLAIN, GLOBAL_OUTPUT],
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

#[cfg(test)]
#[path = "cli_command_registry_graph_records_metadata_tests.rs"]
mod metadata_tests;
