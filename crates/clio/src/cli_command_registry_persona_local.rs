// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the persona and
//! maintenance commands — `persona get`, `persona stable`, `persona observe`,
//! and `maintenance status` — and for the `PERSONA_LOCAL` tranche that lists
//! all nine persona and local-state records in the registry's declared order.
//!
//! The scratchpad and canonical records live in the `workspace` sibling this
//! module declares and re-exports.
//!
//! ## Owns
//! - The four `CommandSpec` records here, the `PERSONA_LOCAL` tranche list,
//!   and the shared flag consts the sibling also uses
//! - The description, value kind, and default of each documented flag
//!
//! ## Does not own
//! - The scratchpad and canonical records (see the `workspace` sibling module)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry lookups and the global order of the rendered surfaces (see
//!   `cli_command_registry` and `cli_command_bindings`)
//! - The command semantics (see the persona, maintenance, and workspace
//!   command surfaces)
//!
//! ## Boundary
//!
//! Changes to one of this group's commands' flags or summary text should
//! normally require changes here. Changes to the command implementations
//! should not.
//!
//! Keep this module focused on the persona and maintenance records and the
//! tranche list.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the tool is the
// binding row, so both rendered surfaces keep their exact output.

use crate::cli_command_registry::{GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, Section};

/// The scratchpad and canonical records: a sibling file so both halves of the
/// tranche stay within the size cap. The records are re-exported below so
/// `PERSONA_LOCAL` and the tranche tests read them by name.
#[path = "cli_command_registry_persona_local_workspace.rs"]
mod workspace;

pub(crate) use workspace::{
    CANONICAL_GET, CANONICAL_PUT, SCRATCHPAD_CLEAR, SCRATCHPAD_READ, SCRATCHPAD_WRITE,
};

/// The required `KEY` positional of `persona stable`.
const KEY: Positional = Positional {
    label: "KEY",
    required: true,
    description: "The persona key to set.",
};

/// The required `KEY` positional of `persona observe`.
const OBSERVE_KEY: Positional = Positional {
    label: "KEY",
    required: true,
    description: "The persona key the observation updates.",
};

/// The required `VALUE` positional of `persona stable`.
const VALUE: Positional = Positional {
    label: "VALUE",
    required: true,
    description: "The persona value stored under the key.",
};

/// The required `NUMBER` positional of `persona observe`.
const NUMBER: Positional = Positional {
    label: "NUMBER",
    required: true,
    description: "The numeric observation fed into the preference's exponential moving average.",
};

/// The `--explain` trace flag this tranche's read commands document.
const EXPLAIN: Flag = Flag::toggle("explain", "Asks `read_args` for the trace detail.");

/// The `--dry-run` preview flag this tranche's write commands document; the
/// `workspace` sibling's `canonical put` also uses it.
const DRY_RUN: Flag = Flag::toggle("dry-run", "Previews with no durable change.");

/// The `--source explicit|inferred` of `persona stable`, which the tool
/// defaults to `explicit`.
const SOURCE_STABLE: Flag = Flag {
    default: Some("explicit"),
    ..Flag::choice(
        "source",
        "explicit|inferred",
        "`optional_flags` validates `explicit` or `inferred`.",
        &["explicit", "inferred"],
    )
};

/// The `--source explicit|inferred` of `persona observe`, which the tool
/// defaults to `inferred`.
const SOURCE_OBSERVE: Flag = Flag {
    default: Some("inferred"),
    ..Flag::choice(
        "source",
        "explicit|inferred",
        "`optional_flags` validates `explicit` or `inferred`.",
        &["explicit", "inferred"],
    )
};

/// The `--confidence N` of `persona stable`, `persona observe`, and the
/// `workspace` sibling's `canonical put`.
const CONFIDENCE: Flag = Flag::float(
    "confidence",
    "N",
    "`parse_unit_f64` bounds it to `0.0..=1.0`.",
);

/// The `--evidence-ref E` of the persona writes and the `workspace` sibling's
/// `canonical put`.
const EVIDENCE_REF: Flag = Flag::text("evidence-ref", "E", "Sets the entry's `evidence_ref`.");

/// The `--context TEXT` of `persona stable` and `persona observe`.
const PERSONA_CONTEXT: Flag = Flag::text("context", "TEXT", "Sets the entry's `context`.");

/// The `--budget-tokens N` of `persona get`, which `clio_persona` defaults to
/// `DEFAULT_PERSONA_TOKEN_BUDGET`.
const BUDGET_TOKENS: Flag = Flag {
    default: Some("400"),
    ..Flag::uint(
        "budget-tokens",
        "N",
        "`persona_get_call` sets the token budget.",
    )
};

/// The `--item-id ID` of `maintenance status`.
const ITEM_ID: Flag = Flag::text(
    "item-id",
    "ID",
    "`maintenance_status_call` sets the `item_id` filter.",
);

/// The global flags these records accept without printing: `--bank` is
/// documented on each usage line, so it is left out here.
const IMPLICIT_GLOBALS: &[&str] = &["db", "backend", "actor", "no-input", "yes"];

/// `persona observe` accepts `--context` without printing it, so its printed
/// line is pinned here while `documented` keeps the parser's accepted set.
const PERSONA_OBSERVE_USAGE: &str = "clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--bank BANK] [--output json|text]";

/// `clio persona get`: the persona document, within a token budget.
pub(crate) const PERSONA_GET: CommandSpec = CommandSpec {
    name: "persona get",
    aliases: &[],
    summary: "the persona model, within a token budget",
    notes: &[],
    positionals: &[],
    documented: &[BUDGET_TOKENS, EXPLAIN, GLOBAL_BANK, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio persona get --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio persona get --budget-tokens 800",
            comment: Some("larger budget"),
        },
        Example {
            command: "clio persona get -o json --db ./memory.db",
            comment: None,
        },
    ],
    tool: Some("persona_get"),
    section: Section::Read,
};

/// `clio persona stable`: set one stable persona key over `persona_put_stable`.
pub(crate) const PERSONA_STABLE: CommandSpec = CommandSpec {
    name: "persona stable",
    aliases: &[],
    summary: "set a stable persona key",
    notes: &[],
    positionals: &[KEY, VALUE],
    documented: &[
        CONFIDENCE,
        SOURCE_STABLE,
        EVIDENCE_REF,
        PERSONA_CONTEXT,
        DRY_RUN,
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio persona stable response_style concise --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio persona stable response_style concise --confidence 0.9 --source explicit",
            comment: None,
        },
        Example {
            command: "clio persona stable response_style concise -o json",
            comment: None,
        },
    ],
    tool: Some("persona_put_stable"),
    section: Section::Write,
};

/// `clio persona observe`: one numeric observation into the EMA over
/// `persona_observe_preference`; `--context` is accepted but never printed.
pub(crate) const PERSONA_OBSERVE: CommandSpec = CommandSpec {
    name: "persona observe",
    aliases: &[],
    summary: "feed one numeric observation into the EMA",
    notes: &[],
    positionals: &[OBSERVE_KEY, NUMBER],
    documented: &[
        CONFIDENCE,
        SOURCE_OBSERVE,
        EVIDENCE_REF,
        PERSONA_CONTEXT,
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: Some(PERSONA_OBSERVE_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio persona observe response_style 0.8 --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio persona observe response_style 0.8 --source inferred --evidence-ref run-42",
            comment: None,
        },
        Example {
            command: "clio persona observe response_style 0.8 -o json",
            comment: None,
        },
    ],
    tool: Some("persona_observe_preference"),
    section: Section::Write,
};

/// `clio maintenance status`: queue depth, failures, and drain progress.
pub(crate) const MAINTENANCE_STATUS: CommandSpec = CommandSpec {
    name: "maintenance status",
    aliases: &[],
    summary: "queue depth, failures, and drain progress",
    notes: &[],
    positionals: &[],
    documented: &[ITEM_ID, EXPLAIN, GLOBAL_BANK, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio maintenance status --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio maintenance status --item-id itm-sample-1 --explain",
            comment: None,
        },
    ],
    tool: Some("maintenance_status"),
    section: Section::Read,
};

/// This module's nine records, in the registry's declared tranche order. The
/// scratchpad and canonical records come from the `workspace` sibling.
pub(crate) static PERSONA_LOCAL: &[CommandSpec] = &[
    PERSONA_GET,
    PERSONA_STABLE,
    PERSONA_OBSERVE,
    SCRATCHPAD_WRITE,
    SCRATCHPAD_READ,
    SCRATCHPAD_CLEAR,
    CANONICAL_PUT,
    CANONICAL_GET,
    MAINTENANCE_STATUS,
];

#[cfg(test)]
#[path = "cli_command_registry_persona_local_tests.rs"]
mod tests;
