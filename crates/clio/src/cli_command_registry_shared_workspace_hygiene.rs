// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the hygiene commands:
//! `hygiene audit`, `hygiene clean`, and `hygiene log`.
//!
//! It is the sibling the `cli_command_registry_shared_workspace` module
//! declares and re-exports, so the tranche's `SHARED_WORKSPACE` list can name
//! these records while each half of the tranche stays within the size cap.
//!
//! ## Owns
//! - The three `CommandSpec` records, the `HYGIENE_RECORDS` list that
//!   groups them, and the flag consts only they use
//! - The description, value kind, and default of each documented flag
//! - The `hygiene clean` alternate line and the two notes under its usage line
//!
//! ## Does not own
//! - The shared-bank, intent, compose, and audit records and the tranche list
//!   (see `cli_command_registry_shared_workspace`)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The command semantics (see the hygiene read and write surfaces)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags or summary text should normally
//! require changes here. Changes to the command implementations should not.
//!
//! Keep this module focused on the hygiene records.

use crate::cli_command_registry::{GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

/// The hygiene records print `--bank`, so only the remaining globals ride along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK: &[&str] = &["db", "backend", "actor", "no-input", "yes"];

/// The closed `--action` set `cli_write_hygiene` validates against.
const HYGIENE_ACTIONS: &[&str] = &["flag", "archive", "discard", "delete", "keep"];

/// The `--limit N` page cap the hygiene read commands document: the tool
/// defaults it to 50 when omitted, and the CLI lowers an above-cap request to
/// 200 instead of the store refusing it.
const HYGIENE_LIMIT: Flag = Flag {
    default: Some("50"),
    ..Flag::uint(
        "limit",
        "N",
        "Caps the page; defaults to 50 when omitted; a limit above 200 is lowered to 200.",
    )
};

/// The `--offset N` page skip the hygiene read commands document.
const HYGIENE_OFFSET: Flag = Flag {
    default: Some("0"),
    ..Flag::uint(
        "offset",
        "N",
        "Skips leading results; `cli_read_hygiene` parses it, and the tool defaults it to 0 when omitted.",
    )
};

/// `hygiene clean` cannot run without a candidates file.
const FILE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text("file", "candidates.json", "`cli_write_hygiene` reads it.")
};

/// The `hygiene clean` alternate: the stdin spelling of `--file`.
const HYGIENE_CLEAN_ALTERNATE: &str =
    "clio hygiene clean --file - (reads the candidates JSON from stdin)";

/// The `hygiene clean` note: what a run without `--confirm` does.
const HYGIENE_CLEAN_NOTE: &str =
    "without --confirm (or with --dry-run) the call is a zero-write dry-run: counts only";

/// The `hygiene clean` note: what `--action keep` does with each candidate.
const HYGIENE_KEEP_NOTE: &str =
    "--action keep applies each candidate's suggested action, including archive or discard";

/// `clio hygiene audit`: score stored items and list the weakest.
pub(crate) const HYGIENE_AUDIT: CommandSpec = CommandSpec {
    name: "hygiene audit",
    aliases: &[],
    summary: "score stored items and list the weakest",
    notes: &[],
    positionals: &[],
    documented: &[
        Flag {
            default: Some("0.40"),
            ..Flag::float(
                "min-score",
                "N",
                "Sets the score floor; `cli_read_hygiene` parses it, and `clio_hygiene` defaults it to `DEFAULT_MIN_SCORE`.",
            )
        },
        HYGIENE_LIMIT,
        HYGIENE_OFFSET,
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio hygiene audit --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio hygiene audit --min-score 0.5 --limit 20",
            comment: None,
        },
        Example {
            command: "clio hygiene audit --bank work -o json",
            comment: None,
        },
    ],
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
        Flag {
            default: Some("keep"),
            ..Flag::choice(
                "action",
                "flag|archive|discard|keep",
                "Sets the action per candidate (`flag`, `archive`, `discard`, `delete`, `keep`); defaults to `keep`.",
                HYGIENE_ACTIONS,
            )
        },
        Flag::toggle("confirm", "`hygiene_clean` applies the batch."),
        Flag::toggle("dry-run", "`hygiene_clean` previews instead."),
        Flag::text("reason", "R", "`hygiene_clean` forwards it."),
        Flag::text("note", "N", "`hygiene_clean` forwards it."),
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[HYGIENE_CLEAN_ALTERNATE],
    examples: &[
        Example {
            command: "clio hygiene clean --file candidates.json --dry-run --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio hygiene clean --file candidates.json --confirm --action keep",
            comment: None,
        },
        Example {
            command: "clio hygiene clean --file candidates.json --action flag --note \"weak item\" --bank work",
            comment: None,
        },
    ],
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
    documented: &[HYGIENE_LIMIT, HYGIENE_OFFSET, GLOBAL_BANK, GLOBAL_OUTPUT],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio hygiene log --db ./memory.db",
            comment: None,
        },
        Example {
            command: "clio hygiene log --limit 20 --offset 40 --bank work",
            comment: None,
        },
    ],
    tool: Some("hygiene_log_list"),
    section: Section::Read,
};

/// The records this file declares, in declaration order: the list the
/// boundary test reads, so a record added to this file must join it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const HYGIENE_RECORDS: &[CommandSpec] = &[HYGIENE_AUDIT, HYGIENE_CLEAN, HYGIENE_LOG];

#[cfg(test)]
#[path = "cli_command_registry_shared_workspace_hygiene_tests.rs"]
mod tests;
