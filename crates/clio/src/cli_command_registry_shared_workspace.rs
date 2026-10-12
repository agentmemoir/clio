// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the shared-bank,
//! intent-gate, context-composition, and audit commands: `shared store`,
//! `shared retrieve`, `shared discard`, `intent gate`, `compose-context`, and
//! `audit trail`, and for the `SHARED_WORKSPACE` tranche that lists all nine
//! shared-workspace records in the registry's declared order.
//!
//! The hygiene records live in the `hygiene` sibling this module declares and
//! re-exports.
//!
//! ## Owns
//! - The six `CommandSpec` records here and the `SHARED_WORKSPACE` tranche list
//! - The description, value kind, and default of each documented flag
//!
//! ## Does not own
//! - The hygiene records (see the `hygiene` sibling module)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry lookups and the global order of the rendered surfaces (see
//!   `cli_command_registry` and `cli_command_bindings`)
//! - The command semantics (see the shared-bank and workspace command surfaces)
//!
//! ## Boundary
//!
//! Changes to one of this group's commands' flags or summary text should
//! normally require changes here. Changes to the command implementations
//! should not.
//!
//! Keep this module focused on the shared-bank, intent, compose, and audit
//! records and the tranche list.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the tool is the
// binding row, so both rendered surfaces keep their exact output.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, Section};
use clio_types::SourceType;

/// The hygiene records: a sibling file so both halves of the tranche stay
/// within the size cap. The records are re-exported below so `SHARED_WORKSPACE`
/// and the tranche tests read them by name.
#[path = "cli_command_registry_shared_workspace_hygiene.rs"]
mod hygiene;

pub(crate) use hygiene::{HYGIENE_AUDIT, HYGIENE_CLEAN, HYGIENE_LOG};

/// The global flags these records accept without printing.
const IMPLICIT_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The shared-bank records print `--actor`, so `--bank` alone rides along.
const IMPLICIT_GLOBALS_OUTSIDE_ACTOR: &[&str] = &["db", "backend", "bank", "no-input", "yes"];

/// The `shared store` body.
const TEXT: Positional = Positional {
    label: "TEXT",
    required: true,
    description: "The text of the item stored in the shared bank.",
};

/// The `shared retrieve` query.
const QUERY: Positional = Positional {
    label: "QUERY",
    required: true,
    description: "The search query for the shared bank.",
};

/// The `shared discard` target.
const DISCARD_ITEM_ID: Positional = Positional {
    label: "ITEM_ID",
    required: true,
    description: "The id of the shared-bank item to remove.",
};

/// The `intent gate` turn text.
const TURN_TEXT: Positional = Positional {
    label: "TURN_TEXT",
    required: true,
    description: "The turn text the gate classifies.",
};

/// The optional `compose-context` query; `--query` replaces it when both are given.
const OPTIONAL_QUERY: Positional = Positional {
    label: "QUERY",
    required: false,
    description: "The query to assemble context for; `--query` replaces it when both are given.",
};

/// The `audit trail` target.
const AUDIT_ITEM_ID: Positional = Positional {
    label: "ITEM_ID",
    required: true,
    description: "The item id whose lifecycle is reconstructed.",
};

/// The `--explain` trace flag the read commands here document.
const EXPLAIN: Flag = Flag::toggle("explain", "`read_args` requests the trace.");

/// The shared-bank lines print the global `--actor ACTOR` placeholder and
/// cannot run without it.
const SHARED_ACTOR_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text("actor", "ACTOR", "`base_args` forwards the identity.")
};

/// The `--shared-bank` opt-in is written bare on every shared record.
const SHARED_BANK_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::toggle("shared-bank", "`cli_write` enables the shared bank.")
};

/// `shared discard` cannot run without its reason.
const REASON_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text("reason", "R", "`shared_discard` forwards the reason.")
};

/// `shared store` cannot run without an epistemic kind.
const EPISTEMIC_KIND_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::choice(
        "epistemic-kind",
        "K",
        "Marks `fact` or `belief`; belief requires `--source-type`.",
        &["fact", "belief"],
    )
};

/// The `--epistemic-kind`/`--source-type` pairing `shared store` prints under
/// its usage line.
const SOURCE_TYPE_PAIRING_NOTE: &str = "--epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact";

/// The `shared store` printed line: the parser's accepted set keeps the
/// `category`-first order the dispatcher already merges, so the line is pinned.
const SHARED_STORE_USAGE: &str = "clio shared store TEXT --epistemic-kind K [--category C] [--episodic-type T] [--source-type T] [--confidence N] [--evidence-ref E] --actor ACTOR --shared-bank [--output json|text]";

/// `compose-context` accepts `--query` without printing it and prints
/// `--budget-tokens` as a mandated flag, so its line is pinned too.
const COMPOSE_CONTEXT_USAGE: &str = "clio compose-context [QUERY] --budget-tokens N [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--recall-scope full|consolidated_only] [--expand-graph] [--prefer-consolidated] [--explain] [--output json|text]";

/// `clio shared store`: write one item into the shared bank.
pub(crate) const SHARED_STORE: CommandSpec = CommandSpec {
    name: "shared store",
    aliases: &[],
    summary: "write into a shared bank",
    notes: &[SOURCE_TYPE_PAIRING_NOTE],
    positionals: &[TEXT],
    documented: &[
        Flag::text("category", "C", "`shared_store` forwards the category."),
        Flag::text("episodic-type", "T", "`shared_store` forwards the kind."),
        EPISTEMIC_KIND_REQUIRED,
        Flag::choice(
            "source-type",
            "T",
            "Names belief provenance (`user_stated`, `agent_inferred`, `third_party`); required with belief.",
            SourceType::WIRE,
        ),
        Flag::float("confidence", "N", "`parse_unit_f64` bounds the value."),
        Flag::text("evidence-ref", "E", "`shared_store` forwards evidence."),
        SHARED_ACTOR_REQUIRED,
        SHARED_BANK_REQUIRED,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_ACTOR,
    flag_aliases: &[],
    usage_override: Some(SHARED_STORE_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio shared store \"onboarding notes\" --category persona --epistemic-kind fact --actor agent --shared-bank --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio shared store \"onboarding notes\" --category persona --epistemic-kind belief --source-type user_stated --actor agent --shared-bank",
            comment: None,
        },
        Example {
            command: "clio shared store \"onboarding notes\" --category persona --epistemic-kind fact --actor agent --shared-bank -o json",
            comment: None,
        },
    ],
    tool: Some("shared_store"),
    section: Section::Write,
};

/// `clio shared retrieve`: hybrid search scoped to the shared bank.
pub(crate) const SHARED_RETRIEVE: CommandSpec = CommandSpec {
    name: "shared retrieve",
    aliases: &[],
    summary: "read from a shared bank",
    notes: &[],
    positionals: &[QUERY],
    documented: &[SHARED_ACTOR_REQUIRED, SHARED_BANK_REQUIRED, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio shared retrieve \"onboarding\" --actor agent --shared-bank --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio shared retrieve \"onboarding\" --actor agent --shared-bank -o json",
            comment: None,
        },
    ],
    tool: Some("shared_retrieve"),
    section: Section::Read,
};

/// `clio shared discard`: preview without `--confirm`, remove with it.
pub(crate) const SHARED_DISCARD: CommandSpec = CommandSpec {
    name: "shared discard",
    aliases: &[],
    summary: "remove one item from a shared bank",
    notes: &[],
    positionals: &[DISCARD_ITEM_ID],
    documented: &[
        REASON_REQUIRED,
        Flag::toggle("confirm", "`shared_discard` previews without it."),
        SHARED_ACTOR_REQUIRED,
        SHARED_BANK_REQUIRED,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio shared discard itm-sample-1 --reason stale --actor agent --shared-bank --db ./memory.db",
            comment: Some("preview"),
        },
        Example {
            command: "clio shared discard itm-sample-1 --reason stale --confirm --actor agent --shared-bank",
            comment: None,
        },
    ],
    tool: Some("shared_discard"),
    section: Section::Write,
};

/// `clio intent gate`: decide whether a turn should reach memory.
pub(crate) const INTENT_GATE: CommandSpec = CommandSpec {
    name: "intent gate",
    aliases: &[],
    summary: "decide whether a turn should reach memory",
    notes: &[],
    positionals: &[TURN_TEXT],
    documented: &[EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio intent gate \"should this turn reach memory?\" --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio intent gate \"should this turn reach memory?\" --explain",
            comment: None,
        },
    ],
    tool: Some("intent_gate"),
    section: Section::Read,
};

/// `clio compose-context`: assemble a budgeted context block for a turn.
pub(crate) const COMPOSE_CONTEXT: CommandSpec = CommandSpec {
    name: "compose-context",
    aliases: &[],
    summary: "assemble a budgeted context block for a turn",
    notes: &[],
    positionals: &[OPTIONAL_QUERY],
    documented: &[
        Flag::text("query", "Q", "`compose_context` accepts either."),
        Flag::text("domains", "a,b", "`split_domains` splits the value."),
        Flag::uint("budget-tokens", "N", "`parse_u64` sets the budget."),
        Flag::text("as-of", "TS", "`compose_context` forwards it."),
        Flag::text("query-anchor", "TS", "`compose_context` forwards it."),
        Flag::choice(
            "time-axis",
            "valid|transaction",
            "Selects the bi-temporal axis (`valid` or `transaction`).",
            &["valid", "transaction"],
        ),
        Flag::choice(
            "recall-scope",
            "full|consolidated_only",
            "Selects the recall scope (`full` or `consolidated_only`).",
            &["full", "consolidated_only"],
        ),
        Flag::toggle("expand-graph", "`compose_context` expands the graph."),
        Flag::toggle("prefer-consolidated", "`compose_context` prefers units."),
        EXPLAIN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: Some(COMPOSE_CONTEXT_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio compose-context --budget-tokens 2000 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio compose-context \"how do I deploy\" --budget-tokens 2000 --domains semantic,task",
            comment: None,
        },
        Example {
            command: "clio compose-context \"how do I deploy\" --budget-tokens 2000 --expand-graph -o json",
            comment: None,
        },
    ],
    tool: Some("compose_context"),
    section: Section::Read,
};

/// `clio audit trail`: masked lifecycle reconstruction for one item.
pub(crate) const AUDIT_TRAIL: CommandSpec = CommandSpec {
    name: "audit trail",
    aliases: &[],
    summary: "masked lifecycle reconstruction for one item",
    notes: &[],
    positionals: &[AUDIT_ITEM_ID],
    documented: &[EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio audit trail itm-sample-1 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio audit trail itm-sample-1 --explain",
            comment: None,
        },
    ],
    tool: Some("audit_trail"),
    section: Section::Read,
};

/// This module's nine records, in the registry's declared tranche order. The
/// hygiene records come from the `hygiene` sibling.
pub(crate) static SHARED_WORKSPACE: &[CommandSpec] = &[
    SHARED_STORE,
    SHARED_RETRIEVE,
    SHARED_DISCARD,
    HYGIENE_AUDIT,
    HYGIENE_CLEAN,
    HYGIENE_LOG,
    INTENT_GATE,
    COMPOSE_CONTEXT,
    AUDIT_TRAIL,
];

#[cfg(test)]
#[path = "cli_command_registry_shared_workspace_tests.rs"]
mod tests;
