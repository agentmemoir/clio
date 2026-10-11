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
//! - The description, value kind, and default of each documented flag
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
    ..Flag::choice(
        "action",
        "attest|update|invalidate|delete",
        "Selects the review action that `cli_write_shared::validate_call` validates against `attest`, `update`, `invalidate`, or `delete` before calling the `validate` tool.",
        &["attest", "update", "invalidate", "delete"],
    )
};

/// `batch` cannot run without its operations file.
const FILE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text(
        "file",
        "ops.json",
        "Names the operations JSON file that `cli_write_batch::batch_call` decodes and hands to the `batch` tool; `-` reads the document from stdin.",
    )
};

/// `export` cannot run without its output file.
const OUT_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text(
        "out",
        "FILE",
        "Names the bundle file that `cli_write_portability::export_call` passes to the `export` tool as `destination`.",
    )
};

/// `import` cannot run without its source bundle.
const SOURCE_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text(
        "source",
        "FILE",
        "Names the bundle file that `cli_write_portability::import_call` passes to the `import` tool as `source`.",
    )
};

/// `erase` cannot run without its subject.
const SUBJECT_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text(
        "subject",
        "S",
        "Names the data subject whose key is destroyed; `cli_write_portability::erase_call` passes it to the `erase_request` tool as `subject_id`.",
    )
};

/// `erase` cannot run without its basis.
const BASIS_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text(
        "basis",
        "B",
        "Records the legal basis for erasure; `cli_write_portability::erase_call` passes it to the `erase_request` tool as `legal_basis`.",
    )
};

/// `erase` cannot run without its request identifier.
const REQUEST_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text(
        "request",
        "R",
        "Identifies the erasure request; `cli_write_portability::erase_call` passes it to the `erase_request` tool as `request_id`.",
    )
};

/// `erase` prints its actor choice, not the global spelling.
const ERASE_ACTOR: Flag = Flag::choice(
    "actor",
    "harness|system",
    "Selects the erasure actor that the `erase_request` tool accepts as `harness` or `system`; `cli_write_portability::erase_call` passes it through `cli_read_args::base_args`.",
    &["harness", "system"],
);

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
        Flag::text(
            "note",
            "N",
            "Carries the reviewer note that `cli_write_shared::validate_call` passes to the `validate` tool as `note`.",
        ),
        Flag::text(
            "new-content",
            "C",
            "Carries the replacement text that `cli_write_shared::validate_call` passes to the `validate` tool as `new_content`.",
        ),
        Flag::text(
            "bank",
            "B",
            "Scopes the review to one bank; `cli_read_args::base_args` passes it to the `validate` tool.",
        ),
        Flag::text(
            "actor",
            "A",
            "Names the acting identity; `cli_read_args::base_args` passes it to the `validate` tool.",
        ),
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
        Flag::toggle(
            "dry-run",
            "Reports the batch plan without writing; `cli_write_batch::batch_call` sets the `batch` tool's `dry_run` argument.",
        ),
        Flag::text(
            "bank",
            "B",
            "Scopes the batch to one bank; `cli_read_args::base_args` passes it to the `batch` tool, not to individual operations.",
        ),
        Flag::text(
            "actor",
            "A",
            "Names the acting identity; `cli_read_args::base_args` passes it to the `batch` tool.",
        ),
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
        Flag {
            default: Some("dsar_plaintext"),
            ..Flag::choice(
                "content-mode",
                "dsar_plaintext|ciphertext_backup",
                "Selects the bundle content mode that `cli_write_portability::export_call` validates against `dsar_plaintext` or `ciphertext_backup`, and the tool defaults it to `dsar_plaintext` when omitted.",
                &["dsar_plaintext", "ciphertext_backup"],
            )
        },
        Flag::choice(
            "category",
            "C",
            "Filters the bundle to one semantic category; `cli_write_portability::export_call` validates it against `clio_types::SemanticCategory::WIRE` (`task_spec`, `schema`, `tool_config`, `output_constraint`, `persona`).",
            clio_types::SemanticCategory::WIRE,
        ),
        Flag::text(
            "time-start",
            "TS",
            "Bounds the bundle to items created at or after this timestamp; `cli_write_portability::export_call` passes it as `time_start`.",
        ),
        Flag::text(
            "time-end",
            "TS",
            "Bounds the bundle to items created at or before this timestamp; `cli_write_portability::export_call` passes it as `time_end`.",
        ),
        Flag {
            default: Some("true"),
            ..Flag::choice(
                "include-archived",
                "true|false",
                "Chooses whether archived items join the bundle; `cli_write_portability::include_flag` validates `true` or `false` and maps it to the tool's boolean, which defaults to true.",
                &["true", "false"],
            )
        },
        Flag {
            default: Some("true"),
            ..Flag::choice(
                "include-discarded",
                "true|false",
                "Chooses whether discarded items join the bundle; `cli_write_portability::include_flag` validates `true` or `false` and maps it to the tool's boolean, which defaults to true.",
                &["true", "false"],
            )
        },
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
        Flag::toggle(
            "dry-run",
            "Previews the restore without writing; `cli_write_portability::import_call` sets the `import` tool's `dry_run` argument.",
        ),
        Flag::toggle(
            "force",
            "Allows overwriting existing items; `cli_write_portability::import_call` sets the `import` tool's `force` argument, which the tool requires with `--reason`.",
        ),
        Flag::text(
            "reason",
            "R",
            "Records why the restore overwrites; `cli_write_portability::import_call` passes it to the `import` tool as `reason`.",
        ),
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
        Flag {
            default: Some("COMPLIANCE_CRYPTO_SHRED"),
            ..Flag::text(
                "reason-code",
                "C",
                "Records the reason code for erasure; `cli_write_portability::erase_call` passes it to the `erase_request` tool as `reason_code`, and `clio_compliance` defaults it to `DEFAULT_REASON_CODE`.",
            )
        },
        Flag::toggle(
            "confirm",
            "Passes the confirmation gate that `cli_confirm::require_confirm` enforces before `cli_write_portability::erase_call` runs the erasure.",
        ),
        Flag::toggle(
            "dry-run",
            "Previews the irreversible erasure with zero writes; `cli_write_portability::erase_call` returns a `WriteCall::Preview`.",
        ),
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
