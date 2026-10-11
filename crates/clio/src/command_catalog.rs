// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio help` command catalog: one entry per
//! command, carrying the command's flags and a short description.
//!
//! ## Owns
//! - The catalog's test-only authored-row references: the eight `mcp` and
//!   `ops` rows the record tests read as their byte reference
//! - The renderer that lays an entry out: wrapped usage line, tool arrow, note
//!
//! ## Does not own
//! - The usage lines themselves (see `cli_help_usage`), which every command's own
//!   `--help` also reads
//! - A registered command's name, summary, or flags (see `cli_command_registry`)
//! - The global order the rows print in (see `cli_command_bindings`)
//! - The help layout around the catalog (see the binary `help_text`)
//! - The machine-readable `help --json` catalog and the command suggestions
//!   (see `cli_help`)
//!
//! ## Boundary
//!
//! Changes to the catalog's layout should normally require changes here. A
//! command's row should not: every row renders from its registry record, in
//! the order `cli_command_bindings` holds. Adding or renaming a flag should
//! not require changes here either: the flag list is rendered from the usage
//! registry, so a flag added there appears here with no edit.
//!
//! Keep this module focused on the catalog text itself.

/// The authored rows the reserved-surface record tests read as their byte
/// reference: the summary text the catalog printed before each record landed,
/// pinned equal to the record's summary. No row here renders — every catalog
/// row resolves to its registry record — so the reference is the one place the
/// pre-record summary bytes are kept.
#[cfg(test)]
pub(crate) const AUTHORED_ROW_REFERENCES: &[(&str, &str)] = &[
    (
        "mcp schema-export",
        "Print/write the tool_schema pack (alias: --path)",
    ),
    ("mcp stdio", "Run the MCP stdio transport"),
    (
        "mcp http",
        "Run the MCP Streamable HTTP transport; with no flag (or a bare --bind) scan 127.0.0.1:{range} for the first free port; an explicit HOST:PORT binds exactly that address",
    ),
    ("ops diagnose", "Read-only install/database health snapshot"),
    (
        "ops verify",
        "Referential-integrity and index-coverage checks",
    ),
    ("ops doctor", "Compose a repair plan (read-only)"),
    (
        "ops repair",
        "Apply allowlisted repairs (live run requires --confirm)",
    ),
    (
        "ops reindex",
        "Rebuild derived indexes (live run requires --confirm); a lexical rebuild replaces legacy plaintext index rows with keyed term digests. Ops exit codes: 0 healthy or completed, 1 domain/runtime failure or findings, 2 usage or repair refusal, 3 reindex failed",
    ),
];

/// Every catalog row in whole-catalog order, resolved by the order accessor:
/// each key prints its registry record's name and summary.
///
/// The order spine holds the one order the catalog always printed, and the
/// records carry the rows, so the two together are the catalog's whole source.
fn catalog_rows() -> impl Iterator<Item = (&'static str, &'static str)> {
    crate::cli_command_bindings::catalog_rows().into_iter()
}

/// Every command the catalog lists, in the order it lists them.
///
/// The help catalog and the tests that check the flag lists agree because both
/// read this one list.
#[cfg(test)]
pub(crate) fn catalog_commands() -> impl Iterator<Item = &'static str> {
    catalog_rows().map(|(command, _)| command)
}

/// The catalog text `clio help` prints under its `Usage:` heading.
///
/// Every entry's flag list is rendered from the usage registry, so this function
/// and `clio <command> --help` cannot disagree about which flags a command takes.
/// A command with no registered usage line says so rather than printing a line
/// with no flags, because a silently empty list reads as "this command takes no
/// flags" and is believed.
pub(crate) fn catalog_text() -> String {
    let mut out = String::new();
    for (command, note) in catalog_rows() {
        let mut usage = crate::cli_help_surfaces::wrapped_usage(command, USAGE_WIDTH, LEAD);
        if let Some(last) = usage.last_mut()
            && let Some(tool) = crate::cli_help::bound_tool_name(command)
        {
            last.push_str(ARROW);
            last.push_str(tool);
        }
        for line in &usage {
            out.push_str(line);
            out.push('\n');
        }
        for line in wrap(note, NOTE_WIDTH - NOTE_INDENT.len()) {
            out.push_str(NOTE_INDENT);
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// The indent every catalog line starts at, so the `clio` words line up.
const LEAD: usize = 2;
/// The column the catalog wraps its usage lines at.
const USAGE_WIDTH: usize = 84;
/// The column the catalog wraps its description lines at, indent included.
const NOTE_WIDTH: usize = 92;
/// The indent a description line carries, aligning it under the command's flags.
const NOTE_INDENT: &str = "       ";
/// The gap between the last flag of a command line and its tool name.
const ARROW: &str = "   -> ";

/// `text` wrapped to `width`, with no indent applied.
///
/// A word longer than `width` is never split: half a flag name names a flag that
/// does not exist.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        if word.is_empty() {
            continue;
        }
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && candidate.len() > width {
            lines.push(std::mem::replace(&mut current, word.to_owned()));
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
#[path = "command_catalog_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "command_catalog_seam_tests.rs"]
mod seam_tests;
