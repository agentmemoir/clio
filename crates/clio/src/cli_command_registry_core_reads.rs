// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the five core read
//! commands: `recall`, `get`, `show`, `inspect`, and `stats`.
//!
//! ## Owns
//! - The five `CommandSpec` records and the tranche that lists them in
//!   whole-catalog order
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The command semantics (see `cli_read_core`)
//!
//! ## Boundary
//!
//! Changes to one of these five commands' flags or summary text should normally
//! require changes here. Changes to the read engine should not.
//!
//! Keep this module focused on the five core read records.

use crate::cli_command_registry::{GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// The `recall` note: what a score floor is, and which flags conflict.
const RECALL_NOTE: &str = "a score floor is a relative filter over this query's own scores, not a calibrated relevance threshold; --min-semantic and --min-keyword prune one retrieval arm each, and --no-entities cannot be combined with --entity-summary; --query-anchor must be RFC 3339 with an explicit UTC offset, and a relative phrase without an anchor contributes nothing (no server-time fallback)";

/// The required `QUERY` positional.
const QUERY: Positional = Positional {
    label: "QUERY",
    required: true,
    description: "",
};

/// The required `ID` positional.
const ID: Positional = Positional {
    label: "ID",
    required: true,
    description: "",
};

/// `--snapshot`, one arm of the snapshot/gist choice.
const SNAPSHOT: Flag = Flag {
    exclusive_group: Some("snapshot-gist"),
    ..Flag::boolean("snapshot")
};

/// `--gist`, the other arm of the snapshot/gist choice.
const GIST: Flag = Flag {
    exclusive_group: Some("snapshot-gist"),
    ..Flag::boolean("gist")
};

/// The flags `get` and `show` document.
const GET_DOCUMENTED: &[Flag] = &[SNAPSHOT, GIST, GLOBAL_OUTPUT];

/// The global flags `get`, `show`, and `inspect` accept without printing.
const READS_EXCEPT_BANK_UNPRINTED: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The global flags `recall` and `stats` accept without printing.
const READS_EXCEPT_BANK_PRINTED: &[&str] = &["db", "backend", "actor", "no-input", "yes"];

/// `clio recall`: one query, score floors, and temporal anchors over `retrieve`.
pub(crate) const RECALL: CommandSpec = CommandSpec {
    name: "recall",
    aliases: &[],
    summary: "score floors are relative filters over this query's own scores, not calibrated thresholds",
    notes: &[RECALL_NOTE],
    positionals: &[QUERY],
    documented: &[
        GLOBAL_BANK,
        Flag::value("limit", "N"),
        Flag::value("domains", "a,b"),
        Flag::value("as-of", "TS"),
        Flag::value("query-anchor", "TS"),
        Flag::value("time-axis", "valid|transaction"),
        Flag::value("min-semantic", "N"),
        Flag::value("min-keyword", "N"),
        Flag::value("min-reranker", "N"),
        Flag::value("min-final", "N"),
        Flag::boolean("no-entities"),
        Flag::boolean("entity-summary"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: READS_EXCEPT_BANK_PRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("retrieve"),
    section: Section::Read,
};

/// `clio get`: fetch one memory by id, over the `get` tool.
pub(crate) const GET: CommandSpec = CommandSpec {
    name: "get",
    aliases: &[],
    summary: "fetch one memory by id",
    notes: &[],
    positionals: &[ID],
    documented: GET_DOCUMENTED,
    accepted_extra: READS_EXCEPT_BANK_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("get"),
    section: Section::Read,
};

/// `clio show`: the visible spelling of `get`, over the same `get` tool.
pub(crate) const SHOW: CommandSpec = CommandSpec {
    name: "show",
    aliases: &[],
    summary: "alias of `get`",
    notes: &[],
    positionals: &[ID],
    documented: GET_DOCUMENTED,
    accepted_extra: READS_EXCEPT_BANK_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("get"),
    section: Section::Read,
};

/// `clio inspect`: metadata-only listing with no decrypted body.
pub(crate) const INSPECT: CommandSpec = CommandSpec {
    name: "inspect",
    aliases: &[],
    summary: "metadata-only listing, no decrypted body",
    notes: &[],
    positionals: &[],
    documented: &[
        Flag::value("filter", "JSON"),
        Flag::value("category", "C"),
        Flag::value("epistemic-kind", "K"),
        Flag::value("validity", "V"),
        Flag::value("time-start", "TS"),
        Flag::value("time-end", "TS"),
        Flag::value("limit", "N"),
        Flag::value("offset", "N"),
        Flag::boolean("include-discarded"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: READS_EXCEPT_BANK_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("inspect"),
    section: Section::Read,
};

/// `clio stats`: counts for one bank, over the `stats` tool.
pub(crate) const STATS: CommandSpec = CommandSpec {
    name: "stats",
    aliases: &[],
    summary: "count memories, entities, and index state in one bank",
    notes: &[],
    positionals: &[],
    documented: &[GLOBAL_BANK, GLOBAL_OUTPUT],
    accepted_extra: READS_EXCEPT_BANK_PRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("stats"),
    section: Section::Read,
};

/// The five core read records, in whole-catalog order.
pub(crate) static CORE_READS: &[CommandSpec] = &[RECALL, GET, SHOW, INSPECT, STATS];

#[cfg(test)]
#[path = "cli_command_registry_core_reads_tests.rs"]
mod tests;
