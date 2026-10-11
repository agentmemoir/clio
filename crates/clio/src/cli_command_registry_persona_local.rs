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

/// The global flags these records accept without printing.
const IMPLICIT_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The scratchpad records print `--bank`/`--actor`, so only these ride along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR: &[&str] = &["db", "backend", "no-input", "yes"];

/// A required `KEY`.
const KEY: Positional = Positional {
    label: "KEY",
    required: true,
    description: "",
};

/// The optional `KEY` of `scratchpad read` and `scratchpad clear`.
const OPTIONAL_KEY: Positional = Positional {
    label: "KEY",
    required: false,
    description: "",
};

/// A required `VALUE`.
const VALUE: Positional = Positional {
    label: "VALUE",
    required: true,
    description: "",
};

/// The `scratchpad write` body.
const CONTENT: Positional = Positional {
    label: "CONTENT",
    required: true,
    description: "",
};

/// The `persona observe` observation.
const NUMBER: Positional = Positional {
    label: "NUMBER",
    required: true,
    description: "",
};

/// The scratchpad lines print `--bank B`; the global's own placeholder is longer.
const SCRATCHPAD_BANK: Flag = Flag::value("bank", "B");

/// The scratchpad and persona write lines print `--actor A`.
const SCRATCHPAD_ACTOR: Flag = Flag::value("actor", "A");

/// `canonical put` cannot run without its selector flags.
const CATEGORY_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("category", "C")
};

/// The second `canonical put` selector.
const EPISTEMIC_KIND_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("epistemic-kind", "K")
};

/// The `--epistemic-kind`/`--source-type` pairing `canonical put` prints under
/// its usage line.
const SOURCE_TYPE_PAIRING_NOTE: &str = "--epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact";

/// `persona observe` accepts `--context` without printing it, so its printed
/// line is pinned here while `documented` keeps the parser's accepted set.
const PERSONA_OBSERVE_USAGE: &str = "clio persona observe KEY NUMBER [--confidence N] [--source explicit|inferred] [--evidence-ref E] [--output json|text]";

/// `clio persona get`: the persona document, within a token budget.
pub(crate) const PERSONA_GET: CommandSpec = CommandSpec {
    name: "persona get",
    aliases: &[],
    summary: "the persona model, within a token budget",
    notes: &[],
    positionals: &[],
    documented: &[
        Flag::value("budget-tokens", "N"),
        Flag::boolean("explain"),
        GLOBAL_OUTPUT,
    ],
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
        Flag::value("confidence", "N"),
        Flag::value("source", "explicit|inferred"),
        Flag::value("evidence-ref", "E"),
        Flag::value("context", "TEXT"),
        Flag::boolean("dry-run"),
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
        Flag::value("confidence", "N"),
        Flag::value("source", "explicit|inferred"),
        Flag::value("evidence-ref", "E"),
        Flag::value("context", "TEXT"),
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
    documented: &[
        Flag::value("session", "S"),
        SCRATCHPAD_BANK,
        SCRATCHPAD_ACTOR,
        GLOBAL_OUTPUT,
    ],
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
    documented: &[
        Flag::value("session", "S"),
        SCRATCHPAD_BANK,
        SCRATCHPAD_ACTOR,
        GLOBAL_OUTPUT,
    ],
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
    documented: &[
        Flag::value("session", "S"),
        SCRATCHPAD_BANK,
        SCRATCHPAD_ACTOR,
        GLOBAL_OUTPUT,
    ],
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
        Flag::value("source-type", "T"),
        Flag::value("confidence", "N"),
        Flag::value("source-text", "TEXT"),
        Flag::value("evidence-ref", "E"),
        Flag::value("context", "C"),
        Flag::boolean("dry-run"),
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
    documented: &[Flag::value("as-of", "TS"), GLOBAL_OUTPUT],
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
    documented: &[
        Flag::value("item-id", "ID"),
        Flag::boolean("explain"),
        GLOBAL_OUTPUT,
    ],
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
