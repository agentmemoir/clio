// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the `compliance`
//! commands: the read-only scope ledger, the content-free review-file export,
//! and the tranche that lists every compliance record in capture order.
//!
//! ## Owns
//! - The `compliance inventory` `CommandSpec` record and the tranche that
//!   lists them, carrying the bytes its authored usage row and catalog row held
//! - The record's repeatable `--copy PATH` declaration, and the pinned usage
//!   line: the frozen line names only `--output`, so the override keeps those
//!   bytes while the options block and the catalog still document `--copy`
//! - The `compliance review-export` `CommandSpec` record and its `--out`,
//!   `--limit`, and `--cursor` flags
//! - The re-export of the ten `compliance` request records
//!   (`request-start`, `request-status`, `request-retry`, `request-copy`,
//!   `request-exception`, `request-events`, `request-close`, `peer-status`,
//!   `peer-retry`, `peer-disposition`) from the `requests` sibling
//!
//! ## Does not own
//! - The ten request records and their flags (see the `requests` sibling
//!   module)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The ledger's semantics (see `clio_compliance`) or the command's dispatch,
//!   decoding, and views (see `compliance_cli` / `compliance_request_cli`)
//!
//! ## Boundary
//!
//! Changes to this command's flags, usage text, or summary should normally
//! require changes here. Changes to the ledger's semantics should not.
//!
//! Keep this module focused on the compliance records.
//
// The record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, so both rendered
// surfaces and the frozen golden capture keep their exact output.

use crate::cli_command_registry::{GLOBAL_ACTOR, GLOBAL_BANK, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

/// The ten subject-request and peer-request records: a sibling file so the
/// module stays within the size cap. The records are re-exported below so
/// `COMPLIANCE_LEAVES` can name them.
#[path = "cli_command_registry_compliance_requests.rs"]
mod requests;

pub(crate) use requests::{
    COMPLIANCE_PEER_DISPOSITION, COMPLIANCE_PEER_RETRY, COMPLIANCE_PEER_STATUS,
    COMPLIANCE_REQUEST_CLOSE, COMPLIANCE_REQUEST_COPY, COMPLIANCE_REQUEST_EVENTS,
    COMPLIANCE_REQUEST_EXCEPTION, COMPLIANCE_REQUEST_RETRY, COMPLIANCE_REQUEST_START,
    COMPLIANCE_REQUEST_STATUS,
};

/// The declared globals `compliance inventory` accepts without printing: the
/// shared parser admits every declared global, so the record that prints only
/// `--output` lists the other six here.
const INVENTORY_UNPRINTED: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The declared globals `compliance review-export` accepts without printing:
/// it documents `--bank`, `--actor`, and `--output`, so the other four ride
/// along.
const REVIEW_EXPORT_UNPRINTED: &[&str] = &["db", "backend", "no-input", "yes"];

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

/// The `compliance review-export` required destination.
const REVIEW_OUT_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text(
        "out",
        "FILE",
        "Names the review file to write; `cli_write_portability::run_review_export` passes it to the `review_export` tool as `destination`, which refuses to overwrite an existing caller file.",
    )
};

/// The optional review page size.
const REVIEW_LIMIT: Flag = Flag::text(
    "limit",
    "N",
    "Bounds the review page to N rows; `cli_write_portability::run_review_export` passes it as `limit`, and the tool applies its own default when omitted.",
);

/// The optional review cursor.
const REVIEW_CURSOR: Flag = Flag::text(
    "cursor",
    "C",
    "Resumes the review after this decimal sequence position; `cli_write_portability::run_review_export` passes it as `cursor`.",
);

/// The review-export note: content-free rows, custody, and the actor gate.
const REVIEW_EXPORT_NOTE: &str = "writes the ordered local review events as content-free rows (sequence position, action id, state) under durable destination custody: the destination is recorded pending before any bytes land, an existing file is never overwritten, and only a harness or system actor may export";

/// `clio compliance review-export`: write the content-free review file.
pub(crate) const COMPLIANCE_REVIEW_EXPORT: CommandSpec = CommandSpec {
    name: "compliance review-export",
    aliases: &[],
    summary: "Write the ordered local review events to a content-free file under destination custody (requires --actor harness|system)",
    notes: &[REVIEW_EXPORT_NOTE],
    positionals: &[],
    documented: &[
        REVIEW_OUT_REQUIRED,
        REVIEW_LIMIT,
        REVIEW_CURSOR,
        GLOBAL_BANK,
        GLOBAL_ACTOR,
        GLOBAL_OUTPUT,
    ],
    accepted_extra: REVIEW_EXPORT_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio compliance review-export --out review.txt --actor harness",
            comment: Some("Write the review file under destination custody."),
        },
        Example {
            command: "clio compliance review-export --out review.txt --limit 100 --actor system -o json",
            comment: Some("Write a bounded review page and emit JSON."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// The twelve compliance records, in capture order.
pub(crate) static COMPLIANCE_LEAVES: &[CommandSpec] = &[
    COMPLIANCE_INVENTORY,
    COMPLIANCE_REVIEW_EXPORT,
    COMPLIANCE_REQUEST_START,
    COMPLIANCE_REQUEST_STATUS,
    COMPLIANCE_REQUEST_RETRY,
    COMPLIANCE_REQUEST_COPY,
    COMPLIANCE_REQUEST_EXCEPTION,
    COMPLIANCE_REQUEST_EVENTS,
    COMPLIANCE_REQUEST_CLOSE,
    COMPLIANCE_PEER_STATUS,
    COMPLIANCE_PEER_RETRY,
    COMPLIANCE_PEER_DISPOSITION,
];

#[cfg(test)]
#[path = "cli_command_registry_compliance_tests.rs"]
mod tests;
