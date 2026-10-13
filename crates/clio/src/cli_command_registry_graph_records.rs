// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the commands headed by
//! `triple` — `triple query`, `triple add`, and `triple end` — and for
//! `GRAPH_RECORDS`, the tranche that lists the triple, belief, graph, and
//! associations records in head-group order.
//!
//! A read head can hold a write subcommand: `triple add` and `triple end`
//! write, but they are classified under their read head like every other
//! record here. The belief, graph, and associations records live in the
//! `belief_graph` sibling this module declares and re-exports.
//!
//! ## Owns
//! - The three `CommandSpec` records here, the `GRAPH_RECORDS` tranche list,
//!   and the shared flag consts the sibling also uses
//! - The positionals and examples of the records here
//! - The description, value kind, and default of each documented flag
//!
//! ## Does not own
//! - The belief, graph, and associations records (see the `belief_graph`
//!   sibling module)
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
//! Keep this module focused on the triple records and the tranche list.

use crate::cli_command_registry::{GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, Section};
use clio_types::SourceType;

/// The belief, graph, and associations records: a sibling file so both halves
/// of the tranche stay within the size cap. The records are re-exported below
/// so `GRAPH_RECORDS` and the tranche tests read them by name.
#[path = "cli_command_registry_graph_records_belief_graph.rs"]
mod belief_graph;

pub(crate) use belief_graph::{
    ASSOCIATIONS, BELIEF_HISTORY, BELIEF_OBSERVE, GRAPH_LINK, GRAPH_QUERY,
};

/// The optional `SUBJECT` positional of `triple query`.
const QUERY_SUBJECT: Positional = Positional {
    label: "SUBJECT",
    required: false,
    description: "Subject pattern to match; omit it to match any subject.",
};

/// The optional `PREDICATE` positional of `triple query`.
const QUERY_PREDICATE: Positional = Positional {
    label: "PREDICATE",
    required: false,
    description: "Predicate pattern to match; omit it to match any predicate.",
};

/// The optional `OBJECT` positional of `triple query`.
const QUERY_OBJECT: Positional = Positional {
    label: "OBJECT",
    required: false,
    description: "Object pattern to match; omit it to match any object.",
};

/// The required `S` positional of `triple add` and `triple end`.
const FACT_SUBJECT: Positional = Positional {
    label: "S",
    required: true,
    description: "Subject term of the fact to write or expire.",
};

/// The required `P` positional of `triple add` and `triple end`.
const FACT_PREDICATE: Positional = Positional {
    label: "P",
    required: true,
    description: "Predicate term of the fact to write or expire.",
};

/// The required `O` positional of `triple add`.
const FACT_OBJECT: Positional = Positional {
    label: "O",
    required: true,
    description: "Object term of the fact to write.",
};

/// The `--as-of TS` timestamp of `triple query` and `belief observe`.
const AS_OF: Flag = Flag::text("as-of", "TS", "Sets the reference `as_of` timestamp.");

/// The `--explain` trace flag the read commands document.
const EXPLAIN: Flag = Flag::toggle("explain", "Asks `read_args` for the trace detail.");

/// The `--dry-run` preview flag the write commands document.
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

/// The declared globals these commands accept without printing: every global
/// but `--output`, which each usage line documents.
const UNPRINTED_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The globals the bank-scoped writes accept without printing. `--bank` is
/// documented, so a reader sees it in the usage line rather than only under
/// `Global flags`.
const UNPRINTED_GLOBALS_OUTSIDE_BANK: &[&str] = &["db", "backend", "actor", "no-input", "yes"];

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
    examples: &[
        Example {
            command: "clio triple query itm-sample-1 related itm-sample-2 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio triple query itm-sample-1 --explain",
            comment: Some("one subject term"),
        },
        Example {
            command: "clio triple query --as-of 2026-01-01 -o json",
            comment: None,
        },
    ],
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
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio triple add itm-sample-1 related itm-sample-2 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio triple add itm-sample-1 related itm-sample-2 --confidence 0.9",
            comment: Some("weighted fact"),
        },
        Example {
            command: "clio triple add itm-sample-1 related itm-sample-2 --epistemic-kind belief --source-type user_stated",
            comment: None,
        },
    ],
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
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: UNPRINTED_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio triple end itm-sample-1 related --valid-until 2026-01-01 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio triple end itm-sample-1 related --valid-until 2026-01-01 --object itm-sample-2",
            comment: Some("match by object too"),
        },
        Example {
            command: "clio triple end itm-sample-1 related --valid-until 2026-01-01 --dry-run",
            comment: None,
        },
    ],
    tool: Some("triple_end"),
    section: Section::Write,
};

/// The eight records, in head-group order: `triple`, `belief`, `graph`, then
/// `associations`. The belief, graph, and associations records come from the
/// `belief_graph` sibling.
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
