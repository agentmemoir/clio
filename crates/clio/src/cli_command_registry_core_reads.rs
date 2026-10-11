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
//! - The description, value kind, and default of each documented flag
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
    ..Flag::toggle(
        "snapshot",
        "Returns the stored snapshot instead of the current item; `cli_read_core::get_call` routes it to `get_snapshot`.",
    )
};

/// `--gist`, the other arm of the snapshot/gist choice.
const GIST: Flag = Flag {
    exclusive_group: Some("snapshot-gist"),
    ..Flag::toggle(
        "gist",
        "Returns the item's gist instead of the full body; `cli_read_core::get_call` routes it to `get_gist`.",
    )
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
        Flag {
            default: Some("10"),
            ..Flag::uint(
                "limit",
                "N",
                "Caps the result page; `cli_read_core::recall_call` parses it with `parse_u64` for `retrieve` and `cli_read_core` echoes `clio_retrieve::DEFAULT_LIMIT` when omitted.",
            )
        },
        Flag::text(
            "domains",
            "a,b",
            "Restricts retrieval to the listed domains; `cli_read_core::recall_call` splits the comma-separated value with `split_domains`.",
        ),
        Flag::text(
            "as-of",
            "TS",
            "Sets the bi-temporal instant the query is evaluated at; `cli_read_core::recall_call` passes it to `retrieve` as `as_of`.",
        ),
        Flag::text(
            "query-anchor",
            "TS",
            "Anchors relative time phrases in the query; `cli_read_core::recall_call` passes the RFC 3339 value to `retrieve` as `query_anchor`.",
        ),
        Flag::choice(
            "time-axis",
            "valid|transaction",
            "Selects the bi-temporal axis; `cli_read_args::validate_time_axis` accepts `valid` or `transaction` for `retrieve`.",
            &["valid", "transaction"],
        ),
        Flag::float(
            "min-semantic",
            "N",
            "Prunes the semantic arm below this score floor; `cli_read_core::min_scores_arg` parses it with `parse_f64` for `retrieve`.",
        ),
        Flag::float(
            "min-keyword",
            "N",
            "Prunes the keyword arm below this score floor; `cli_read_core::min_scores_arg` parses it with `parse_f64` for `retrieve`.",
        ),
        Flag::float(
            "min-reranker",
            "N",
            "Filters the ranked page below this reranker score floor; `cli_read_core::min_scores_arg` parses it with `parse_f64`.",
        ),
        Flag::float(
            "min-final",
            "N",
            "Filters the ranked page below this final score floor; `cli_read_core::min_scores_arg` parses it with `parse_f64`.",
        ),
        Flag::toggle(
            "no-entities",
            "Suppresses entity expansion; `cli_read_core::recall_call` sends `include_entities` as false to `retrieve`.",
        ),
        Flag::toggle(
            "entity-summary",
            "Adds the entity summary; `cli_read_core::recall_call` sends `entity_summary` as true to `retrieve`.",
        ),
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
        Flag::text(
            "filter",
            "JSON",
            "Supplies the inspect filter as a JSON object; `cli_read_core::parse_filter` accepts the keys category, epistemic_kind, validity, time_start, time_end, and include_discarded.",
        ),
        Flag::text(
            "category",
            "C",
            "Filters by semantic category; `cli_read_core::inspect_call` adds it to the `inspect` filter.",
        ),
        Flag::text(
            "epistemic-kind",
            "K",
            "Filters by epistemic kind; `cli_read_core::inspect_call` adds it to the `inspect` filter as `epistemic_kind`.",
        ),
        Flag::choice(
            "validity",
            "V",
            "Filters by validity; `cli_read_core::inspect_call` accepts `all`, `active`, `invalidated`, or `discarded`.",
            &["all", "active", "invalidated", "discarded"],
        ),
        Flag::text(
            "time-start",
            "TS",
            "Sets the lower time bound; `cli_read_core::inspect_call` adds it to the `inspect` filter as `time_start`.",
        ),
        Flag::text(
            "time-end",
            "TS",
            "Sets the upper time bound; `cli_read_core::inspect_call` adds it to the `inspect` filter as `time_end`.",
        ),
        Flag {
            default: Some("50"),
            ..Flag::uint(
                "limit",
                "N",
                "Caps the result page; `cli_read_core::inspect_call` parses it with `parse_u64` for `inspect`, which defaults it to 50 when omitted.",
            )
        },
        Flag {
            default: Some("0"),
            ..Flag::uint(
                "offset",
                "N",
                "Skips this many leading results; `cli_read_core::inspect_call` parses it with `parse_u64` for `inspect`, which defaults it to 0 when omitted.",
            )
        },
        Flag::toggle(
            "include-discarded",
            "Includes discarded memories in the listing; `cli_read_core::inspect_call` adds `include_discarded` as true to the filter.",
        ),
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
