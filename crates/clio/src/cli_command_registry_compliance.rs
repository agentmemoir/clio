// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static record of the one `compliance`
//! command: `compliance inventory`, the read-only scope ledger's local surface.
//!
//! ## Owns
//! - The `compliance inventory` `CommandSpec` record and the tranche that
//!   lists it, carrying the bytes its authored usage row and catalog row held
//! - The record's repeatable `--copy PATH` declaration, and the pinned usage
//!   line: the frozen line names only `--output`, so the override keeps those
//!   bytes while the options block and the catalog still document `--copy`
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The ledger's semantics (see `clio_compliance`) or the command's dispatch,
//!   decoding, and views (see `compliance_cli`)
//!
//! ## Boundary
//!
//! Changes to this command's flags, usage text, or summary should normally
//! require changes here. Changes to the ledger's semantics should not.
//!
//! Keep this module focused on the one compliance record.
//
// The record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, so both rendered
// surfaces and the frozen golden capture keep their exact output.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

/// The declared globals `compliance inventory` accepts without printing: the
/// shared parser admits every declared global, so the record that prints only
/// `--output` lists the other six here.
const INVENTORY_UNPRINTED: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The repeatable `--copy PATH` of `compliance inventory`: each occurrence
/// declares one caller-held copy, and `compliance_cli` hands every declared
/// path to `clio_compliance::scope_inventory`, whose copy rows record the path
/// and read it with `std::fs::metadata` only — nothing is created or removed.
const COPY: Flag = Flag {
    repeatable: true,
    ..Flag::text(
        "copy",
        "PATH",
        "Declares one caller-held copy; `compliance_cli` passes every declared path to `clio_compliance::scope_inventory`, whose copy rows record it and never touch it (the path is read with `std::fs::metadata` only).",
    )
};

/// The `compliance inventory` usage line, pinned whole: the frozen surface
/// documents `--output` on the line and `--copy` in the options block, so the
/// assembled order of documented flags cannot write it.
const INVENTORY_USAGE: &str = "clio compliance inventory [--output json|text]";

/// The inventory note: what one run reads, what stays open, and the copy rule.
const INVENTORY_NOTE: &str = "reads the stored record families and their subject links against the live store and reports one row per family with its source, link or named gap, custody, and retention action; a row whose link or source is missing stays open and is named, the complete flag stays false while anything is open or unclassified, and every --copy PATH declares a caller-held copy that is recorded and never touched";

/// `clio compliance inventory`: the read-only scope ledger.
pub(crate) const COMPLIANCE_INVENTORY: CommandSpec = CommandSpec {
    name: "compliance inventory",
    aliases: &[],
    summary: "Read the stored record families and known copies: one row per family with its source, subject link or named gap, custody, and retention action; open rows stay open and named, declared caller copies are recorded and never touched, and nothing is written",
    notes: &[INVENTORY_NOTE],
    positionals: &[],
    documented: &[COPY, GLOBAL_OUTPUT],
    accepted_extra: INVENTORY_UNPRINTED,
    flag_aliases: &[],
    usage_override: Some(INVENTORY_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio compliance inventory",
            comment: Some("Report the stored families and known copies."),
        },
        Example {
            command: "clio compliance inventory -o json",
            comment: Some("Emit the serialized scope inventory as JSON."),
        },
        Example {
            command: "clio compliance inventory --copy ./exports/a.jsonl",
            comment: Some("Record one caller-held copy beside the store's rows."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// The one compliance record, in capture order.
pub(crate) static COMPLIANCE_LEAVES: &[CommandSpec] = &[COMPLIANCE_INVENTORY];

#[cfg(test)]
#[path = "cli_command_registry_compliance_tests.rs"]
mod tests;
