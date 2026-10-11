// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the operation-write
//! commands: `validate`, `batch`, `export`, `import`, and `erase`.
//!
//! ## Owns
//! - The five `CommandSpec` records and the `WRITE_OPERATIONS` tranche that
//!   lists them in whole-catalog order
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The command semantics (see `cli_write_shared`, `cli_write_batch`, and
//!   `cli_write_portability`)
//!
//! ## Boundary
//!
//! Changes to one of these five commands' flags or summary text should
//! normally require changes here. Changes to the write engine should not.
//!
//! Keep this module focused on the five operation-write records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the tool is the
// binding row, so both rendered surfaces keep their exact output.

use crate::cli_command_registry::{GLOBAL_ACTOR, GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// These records print `--bank`/`--actor`, so only these ride along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR: &[&str] = &["db", "backend", "no-input", "yes"];

/// The `import` line prints `--bank` but not `--actor`, so `--actor` rides along.
const IMPLICIT_GLOBALS_OUTSIDE_BANK: &[&str] = &["db", "backend", "actor", "no-input", "yes"];

/// The required `ITEM_ID` positional of `validate`.
const ITEM_ID: Positional = Positional {
    label: "ITEM_ID",
    required: true,
    description: "",
};

/// `validate` cannot run without its action choice.
const ACTION_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("action", "attest|update|invalidate|delete")
};

/// `batch` cannot run without its operations file.
const FILE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("file", "ops.json")
};

/// `export` cannot run without its output file.
const OUT_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("out", "FILE")
};

/// `import` cannot run without its source bundle.
const SOURCE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("source", "FILE")
};

/// `erase` cannot run without its subject.
const SUBJECT_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("subject", "S")
};

/// `erase` cannot run without its basis.
const BASIS_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("basis", "B")
};

/// `erase` cannot run without its request identifier.
const REQUEST_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::value("request", "R")
};

/// `erase` prints its actor choice, not the global spelling.
const ERASE_ACTOR: Flag = Flag::value("actor", "harness|system");

/// The `batch` alternate: the stdin spelling of `--file`.
const BATCH_STDIN_ALTERNATE: &str = "clio batch --file - (reads operations JSON from stdin)";

/// The `erase` printed line: the accepted `--dry-run` never prints, so the
/// line is pinned.
const ERASE_USAGE: &str = "clio erase --subject S --basis B --request R [--reason-code C] --confirm [--actor harness|system] [--bank BANK] [--output json|text]";

/// The `erase` note: what compliance erasure is and is not.
const ERASE_NOTE: &str = "compliance erasure is IRREVERSIBLE (DEK destruction) and is distinct from `discard` and hygiene cleanup";

/// `clio validate`: attest, update, invalidate, or delete one item.
pub(crate) const VALIDATE: CommandSpec = CommandSpec {
    name: "validate",
    aliases: &[],
    summary: "attest, update, invalidate, or delete one item",
    notes: &[],
    positionals: &[ITEM_ID],
    documented: &[
        ACTION_REQUIRED,
        Flag::value("note", "N"),
        Flag::value("new-content", "C"),
        Flag::value("bank", "B"),
        Flag::value("actor", "A"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("validate"),
    section: Section::Write,
};

/// `clio batch`: apply a list of operations in one transaction; `--file -`
/// reads the operations JSON from stdin.
pub(crate) const BATCH: CommandSpec = CommandSpec {
    name: "batch",
    aliases: &[],
    summary: "apply a list of operations in one transaction",
    notes: &[],
    positionals: &[],
    documented: &[
        FILE_REQUIRED,
        Flag::boolean("dry-run"),
        Flag::value("bank", "B"),
        Flag::value("actor", "A"),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[BATCH_STDIN_ALTERNATE],
    examples: &[],
    tool: Some("batch"),
    section: Section::Write,
};

/// `clio export`: write a manifest-backed JSON bundle of stored memories.
pub(crate) const EXPORT: CommandSpec = CommandSpec {
    name: "export",
    aliases: &[],
    summary: "(manifest-backed JSON bundle; requires --actor harness|system)",
    notes: &[],
    positionals: &[],
    documented: &[
        OUT_REQUIRED,
        Flag::value("content-mode", "dsar_plaintext|ciphertext_backup"),
        Flag::value("category", "C"),
        Flag::value("time-start", "TS"),
        Flag::value("time-end", "TS"),
        Flag::value("include-archived", "true|false"),
        Flag::value("include-discarded", "true|false"),
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("export"),
    section: Section::Write,
};

/// `clio import`: load a bundle produced by `clio export`.
pub(crate) const IMPORT: CommandSpec = CommandSpec {
    name: "import",
    aliases: &[],
    summary: "load a bundle produced by `clio export`",
    notes: &[],
    positionals: &[],
    documented: &[
        SOURCE_REQUIRED,
        Flag::boolean("dry-run"),
        Flag::boolean("force"),
        Flag::value("reason", "R"),
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("import"),
    section: Section::Write,
};

/// `clio erase`: irreversible compliance erasure behind the confirm gate;
/// the accepted `--dry-run` never prints, so the line is pinned.
pub(crate) const ERASE: CommandSpec = CommandSpec {
    name: "erase",
    aliases: &[],
    summary: "IRREVERSIBLE compliance erasure (DEK destruction); distinct from `discard` and hygiene cleanup",
    notes: &[ERASE_NOTE],
    positionals: &[],
    documented: &[
        SUBJECT_REQUIRED,
        BASIS_REQUIRED,
        REQUEST_REQUIRED,
        Flag::value("reason-code", "C"),
        Flag::boolean("confirm"),
        Flag::boolean("dry-run"),
        ERASE_ACTOR,
        GLOBAL_BANK,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: IMPLICIT_GLOBALS_OUTSIDE_BANK_ACTOR,
    flag_aliases: &[],
    usage_override: Some(ERASE_USAGE),
    usage_alternates: &[],
    examples: &[],
    tool: Some("erase_request"),
    section: Section::Write,
};

/// The five operation-write records, in whole-catalog order.
pub(crate) static WRITE_OPERATIONS: &[CommandSpec] = &[VALIDATE, BATCH, EXPORT, IMPORT, ERASE];

#[cfg(test)]
#[path = "cli_command_registry_write_operations_tests.rs"]
mod tests;
