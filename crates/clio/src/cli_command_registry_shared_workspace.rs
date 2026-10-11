// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the shared-workspace
//! commands and for the tranche that lists them.
//!
//! ## Owns
//! - The nine `CommandSpec` records and `SHARED_WORKSPACE`, the tranche that
//!   lists them in the registry's declared order
//!
//! ## Does not own
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
//! Keep this module focused on the shared-workspace records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the tool is the
// binding row, so both rendered surfaces keep their exact output.

use crate::cli_command_registry::{GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// The global flags these records accept without printing.
const IMPLICIT_GLOBALS: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The shared-bank records print `--actor`, so `--bank` alone rides along.
const IMPLICIT_GLOBALS_OUTSIDE_ACTOR: &[&str] = &["db", "backend", "bank", "no-input", "yes"];

/// The hygiene records print `--bank`, so only the remaining globals ride along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK: &[&str] = &["db", "backend", "actor", "no-input", "yes"];

/// The `shared store` body.
const TEXT: Positional = Positional {
    label: "TEXT",
    required: true,
    description: "",
};

/// The `shared retrieve` query.
const QUERY: Positional = Positional {
    label: "QUERY",
    required: true,
    description: "",
};

/// The `shared discard` target.
const ITEM_ID: Positional = Positional {
    label: "ITEM_ID",
    required: true,
    description: "",
};

/// The `intent gate` turn text.
const TURN_TEXT: Positional = Positional {
    label: "TURN_TEXT",
    required: true,
    description: "",
};

/// The optional `compose-context` query.
const OPTIONAL_QUERY: Positional = Positional {
    label: "QUERY",
    required: false,
    description: "",
};

/// The shared-bank lines print `--actor A` bare and cannot run without it.
const SHARED_ACTOR_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("actor", "A")
};

/// The `--shared-bank` opt-in is written bare on every shared record.
const SHARED_BANK_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::boolean("shared-bank")
};

/// `shared discard` cannot run without its reason.
const REASON_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("reason", "R")
};

/// `shared store` cannot run without an epistemic kind.
const EPISTEMIC_KIND_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("epistemic-kind", "K")
};

/// `hygiene clean` cannot run without a candidates file.
const FILE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("file", "candidates.json")
};

/// The `--epistemic-kind`/`--source-type` pairing `shared store` prints under
/// its usage line.
const SOURCE_TYPE_PAIRING_NOTE: &str = "--epistemic-kind and --source-type are a pair: --source-type (user_stated|agent_inferred|third_party) is required with belief and must be omitted with fact";

/// The `shared store` printed line: the parser's accepted set keeps the
/// `category`-first order the dispatcher already merges, so the line is pinned.
const SHARED_STORE_USAGE: &str = "clio shared store TEXT --epistemic-kind K [--category C] [--episodic-type T] [--source-type T] [--confidence N] [--evidence-ref E] --actor A --shared-bank [--output json|text]";

/// `compose-context` accepts `--query` without printing it and prints
/// `--budget-tokens` as a mandated flag, so its line is pinned too.
const COMPOSE_CONTEXT_USAGE: &str = "clio compose-context [QUERY] --budget-tokens N [--domains a,b] [--as-of TS] [--query-anchor TS] [--time-axis valid|transaction] [--recall-scope full|consolidated_only] [--expand-graph] [--prefer-consolidated] [--explain] [--output json|text]";

/// The `hygiene clean` alternate: the stdin spelling of `--file`.
const HYGIENE_CLEAN_ALTERNATE: &str =
    "clio hygiene clean --file - (reads the candidates JSON from stdin)";

/// The `hygiene clean` note: what a run without `--confirm` does.
const HYGIENE_CLEAN_NOTE: &str =
    "without --confirm (or with --dry-run) the call is a zero-write dry-run: counts only";

/// The `hygiene clean` note: what `--action keep` does with each candidate.
const HYGIENE_KEEP_NOTE: &str =
    "--action keep applies each candidate's suggested action, including archive or discard";

/// `clio shared store`: write one item into the shared bank.
pub(crate) const SHARED_STORE: CommandSpec = CommandSpec {
    name: "shared store",
    aliases: &[],
    summary: "write into a shared bank",
    notes: &[SOURCE_TYPE_PAIRING_NOTE],
    positionals: &[TEXT],
    documented: &[
        Flag::value("category", "C"),
        Flag::value("episodic-type", "T"),
        EPISTEMIC_KIND_REQUIRED,
        Flag::value("source-type", "T"),
        Flag::value("confidence", "N"),
        Flag::value("evidence-ref", "E"),
        SHARED_ACTOR_REQUIRED,
        SHARED_BANK_REQUIRED,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_ACTOR,
    flag_aliases: &[],
    usage_override: Some(SHARED_STORE_USAGE),
    usage_alternates: &[],
    examples: &[],
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
    examples: &[],
    tool: Some("shared_retrieve"),
    section: Section::Read,
};

/// `clio shared discard`: preview without `--confirm`, remove with it.
pub(crate) const SHARED_DISCARD: CommandSpec = CommandSpec {
    name: "shared discard",
    aliases: &[],
    summary: "remove one item from a shared bank",
    notes: &[],
    positionals: &[ITEM_ID],
    documented: &[
        REASON_REQUIRED,
        Flag::boolean("confirm"),
        SHARED_ACTOR_REQUIRED,
        SHARED_BANK_REQUIRED,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("shared_discard"),
    section: Section::Write,
};

/// `clio hygiene audit`: score stored items and list the weakest.
pub(crate) const HYGIENE_AUDIT: CommandSpec = CommandSpec {
    name: "hygiene audit",
    aliases: &[],
    summary: "score stored items and list the weakest",
    notes: &[],
    positionals: &[],
    documented: &[
        Flag::value("min-score", "N"),
        Flag::value("limit", "N"),
        Flag::value("offset", "N"),
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("hygiene_audit"),
    section: Section::Read,
};

/// `clio hygiene clean`: apply an audit's candidates, zero writes without
/// `--confirm`.
pub(crate) const HYGIENE_CLEAN: CommandSpec = CommandSpec {
    name: "hygiene clean",
    aliases: &[],
    summary: "(or --file - for stdin; without --confirm, or with --dry-run: zero-write dry-run only)",
    notes: &[HYGIENE_CLEAN_NOTE, HYGIENE_KEEP_NOTE],
    positionals: &[],
    documented: &[
        FILE_REQUIRED,
        Flag::value("action", "flag|archive|discard|keep"),
        Flag::boolean("confirm"),
        Flag::boolean("dry-run"),
        Flag::value("reason", "R"),
        Flag::value("note", "N"),
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[HYGIENE_CLEAN_ALTERNATE],
    examples: &[],
    tool: Some("hygiene_clean"),
    section: Section::Write,
};

/// `clio hygiene log`: what hygiene has already done.
pub(crate) const HYGIENE_LOG: CommandSpec = CommandSpec {
    name: "hygiene log",
    aliases: &[],
    summary: "what hygiene has already done",
    notes: &[],
    positionals: &[],
    documented: &[
        Flag::value("limit", "N"),
        Flag::value("offset", "N"),
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("hygiene_log_list"),
    section: Section::Read,
};

/// `clio intent gate`: decide whether a turn should reach memory.
pub(crate) const INTENT_GATE: CommandSpec = CommandSpec {
    name: "intent gate",
    aliases: &[],
    summary: "decide whether a turn should reach memory",
    notes: &[],
    positionals: &[TURN_TEXT],
    documented: &[Flag::boolean("explain"), GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
        Flag::value("query", "Q"),
        Flag::value("domains", "a,b"),
        Flag::value("budget-tokens", "N"),
        Flag::value("as-of", "TS"),
        Flag::value("query-anchor", "TS"),
        Flag::value("time-axis", "valid|transaction"),
        Flag::value("recall-scope", "full|consolidated_only"),
        Flag::boolean("expand-graph"),
        Flag::boolean("prefer-consolidated"),
        Flag::boolean("explain"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: Some(COMPOSE_CONTEXT_USAGE),
    usage_alternates: &[],
    examples: &[],
    tool: Some("compose_context"),
    section: Section::Read,
};

/// `clio audit trail`: masked lifecycle reconstruction for one item.
pub(crate) const AUDIT_TRAIL: CommandSpec = CommandSpec {
    name: "audit trail",
    aliases: &[],
    summary: "masked lifecycle reconstruction for one item",
    notes: &[],
    positionals: &[ITEM_ID],
    documented: &[Flag::boolean("explain"), GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("audit_trail"),
    section: Section::Read,
};

/// This module's nine records, in the registry's declared tranche order.
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
