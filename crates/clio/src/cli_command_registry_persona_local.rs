// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the persona and
//! local-state commands and for the tranche that lists them.
//!
//! ## Owns
//! - The nine `CommandSpec` records and `PERSONA_LOCAL`, the tranche that
//!   lists them in the registry's declared order
//! - The description, value kind, and default of each documented flag
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry lookups and the global order of the rendered surfaces (see
//!   `cli_command_registry` and `cli_command_bindings`)
//! - The command semantics (see the persona and local-state command surfaces)
//!
//! ## Boundary
//!
//! Changes to one of this group's commands' flags or summary text should
//! normally require changes here. Changes to the command implementations
//! should not.
//!
//! Keep this module focused on the persona and local-state records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the tool is the
// binding row, so both rendered surfaces keep their exact output.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};
use clio_types::{SemanticCategory, SourceType};

/// The required `KEY` positional of the persona and scratchpad writes.
const KEY: Positional = Positional {
    label: "KEY",
    required: true,
    description: "",
};

/// The required `VALUE` positional of `persona stable` and `canonical put`.
const VALUE: Positional = Positional {
    label: "VALUE",
    required: true,
    description: "",
};

/// The required `NUMBER` positional of `persona observe`.
const NUMBER: Positional = Positional {
    label: "NUMBER",
    required: true,
    description: "",
};

/// The required `CONTENT` positional of `scratchpad write`.
const CONTENT: Positional = Positional {
    label: "CONTENT",
    required: true,
    description: "",
};

/// The optional `KEY` positional of the scratchpad reads.
const OPTIONAL_KEY: Positional = Positional {
    label: "KEY",
    required: false,
    description: "",
};

/// The `--explain` trace flag this tranche's read commands document.
const EXPLAIN: Flag = Flag::toggle("explain", "Asks `read_args` for the trace detail.");

/// The `--dry-run` preview flag this tranche's write commands document.
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

/// The `--confidence N` of `persona stable`, `persona observe`, and `canonical put`.
const CONFIDENCE: Flag = Flag::float(
    "confidence",
    "N",
    "`parse_unit_f64` bounds it to `0.0..=1.0`.",
);

/// The `--evidence-ref E` of the persona and canonical writes.
const EVIDENCE_REF: Flag = Flag::text("evidence-ref", "E", "Sets the entry's `evidence_ref`.");

/// The `--context TEXT` of `persona stable` and `persona observe`.
const PERSONA_CONTEXT: Flag = Flag::text("context", "TEXT", "Sets the entry's `context`.");

/// The `--session S` of the scratchpad commands.
const SESSION: Flag = Flag::text("session", "S", "`insert_session` sets the session id.");

/// The scratchpad lines print `--bank B`; the global's own placeholder is longer.
const SCRATCHPAD_BANK: Flag = Flag::text("bank", "B", "`base_args` scopes the call to one bank.");

/// The scratchpad and persona write lines print `--actor A`.
const SCRATCHPAD_ACTOR: Flag = Flag::text("actor", "A", "`base_args` names the acting identity.");

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

/// `canonical put` cannot run without its selector flags.
const CATEGORY_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::choice(
        "category",
        "C",
        "`canonical_put_call` requires the slot category from `task_spec|schema|tool_config|output_constraint|persona`.",
        SemanticCategory::WIRE,
    )
};

/// The second `canonical put` selector.
const EPISTEMIC_KIND_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::choice(
        "epistemic-kind",
        "K",
        "`canonical_put_call` requires and validates `fact` or `belief`.",
        &["fact", "belief"],
    )
};

/// The `--source-type T` of `canonical put`.
const SOURCE_TYPE: Flag = Flag::choice(
    "source-type",
    "T",
    "`source_type_arg` requires it with `belief`: `user_stated|agent_inferred|third_party`.",
    SourceType::WIRE,
);

/// The `--source-text TEXT` of `canonical put`.
const SOURCE_TEXT: Flag = Flag::text(
    "source-text",
    "TEXT",
    "`canonical_put_call` sets the `source_text`.",
);

/// The `--context C` of `canonical put`.
const CANONICAL_CONTEXT: Flag = Flag::text(
    "context",
    "C",
    "`context_arg` bounds the context to 4096 bytes.",
);

/// The `--as-of TS` of `canonical get`.
const AS_OF: Flag = Flag::text(
    "as-of",
    "TS",
    "`canonical_get_call` sets the point-in-time `as_of`.",
);

/// The `--item-id ID` of `maintenance status`.
const ITEM_ID: Flag = Flag::text(
    "item-id",
    "ID",
    "`maintenance_status_call` sets the `item_id` filter.",
);

/// The `--epistemic-kind`/`--source-type` pairing `canonical put` prints under
/// its usage line.
const SOURCE_TYPE_PAIRING_NOTE: &str = "--epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact";

/// `persona observe` accepts `--context` without printing it, so its printed
/// line is pinned here while `documented` keeps the parser's accepted set.
const PERSONA_OBSERVE_USAGE: &str = "clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--output json|text]";

/// The global flags these records accept without printing.
const IMPLICIT_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The scratchpad records print `--bank`/`--actor`, so only these ride along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR: &[&str] = &["db", "backend", "no-input", "yes"];

/// `clio persona get`: the persona document, within a token budget.
pub(crate) const PERSONA_GET: CommandSpec = CommandSpec {
    name: "persona get",
    aliases: &[],
    summary: "the persona model, within a token budget",
    notes: &[],
    positionals: &[],
    documented: &[BUDGET_TOKENS, EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
    positionals: &[KEY, NUMBER],
    documented: &[
        CONFIDENCE,
        SOURCE_OBSERVE,
        EVIDENCE_REF,
        PERSONA_CONTEXT,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: Some(PERSONA_OBSERVE_USAGE),
    usage_alternates: &[],
    examples: &[],
    tool: Some("persona_observe_preference"),
    section: Section::Write,
};

/// `clio scratchpad write`: store one ephemeral session key.
pub(crate) const SCRATCHPAD_WRITE: CommandSpec = CommandSpec {
    name: "scratchpad write",
    aliases: &[],
    summary: "store an ephemeral key for this session",
    notes: &[],
    positionals: &[KEY, CONTENT],
    documented: &[SESSION, SCRATCHPAD_BANK, SCRATCHPAD_ACTOR, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("scratchpad_write"),
    section: Section::Write,
};

/// `clio scratchpad read`: one key, or the whole session pad.
pub(crate) const SCRATCHPAD_READ: CommandSpec = CommandSpec {
    name: "scratchpad read",
    aliases: &[],
    summary: "read one key, or the whole pad",
    notes: &[],
    positionals: &[OPTIONAL_KEY],
    documented: &[SESSION, SCRATCHPAD_BANK, SCRATCHPAD_ACTOR, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("scratchpad_read"),
    section: Section::Read,
};

/// `clio scratchpad clear`: drop one key, or the whole session pad.
pub(crate) const SCRATCHPAD_CLEAR: CommandSpec = CommandSpec {
    name: "scratchpad clear",
    aliases: &[],
    summary: "drop one key, or the whole pad",
    notes: &[],
    positionals: &[OPTIONAL_KEY],
    documented: &[SESSION, SCRATCHPAD_BANK, SCRATCHPAD_ACTOR, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("scratchpad_clear"),
    section: Section::Write,
};

/// `clio canonical put`: gated single-slot upsert over `canonical_put`.
pub(crate) const CANONICAL_PUT: CommandSpec = CommandSpec {
    name: "canonical put",
    aliases: &[],
    summary: "gated single-slot upsert with bi-temporal supersession",
    notes: &[SOURCE_TYPE_PAIRING_NOTE],
    positionals: &[KEY, VALUE],
    documented: &[
        CATEGORY_REQUIRED,
        EPISTEMIC_KIND_REQUIRED,
        SOURCE_TYPE,
        CONFIDENCE,
        SOURCE_TEXT,
        EVIDENCE_REF,
        CANONICAL_CONTEXT,
        DRY_RUN,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("canonical_put"),
    section: Section::Write,
};

/// `clio canonical get`: the active or point-in-time canonical slot.
pub(crate) const CANONICAL_GET: CommandSpec = CommandSpec {
    name: "canonical get",
    aliases: &[],
    summary: "read one canonical key",
    notes: &[],
    positionals: &[KEY],
    documented: &[AS_OF, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("canonical_get"),
    section: Section::Read,
};

/// `clio maintenance status`: queue depth, failures, and drain progress.
pub(crate) const MAINTENANCE_STATUS: CommandSpec = CommandSpec {
    name: "maintenance status",
    aliases: &[],
    summary: "queue depth, failures, and drain progress",
    notes: &[],
    positionals: &[],
    documented: &[ITEM_ID, EXPLAIN, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("maintenance_status"),
    section: Section::Read,
};

/// This module's nine records, in the registry's declared tranche order.
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
