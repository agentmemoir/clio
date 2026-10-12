// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-command CLI usage strings shown by
//! `clio <verb> --help` and by the help catalog: the lookup over the registry
//! records.
//!
//! ## Owns
//! - `usage_for` and `usage_line`, the one usage lookup every command's
//!   `--help` and the catalog read
//! - `help_for`, the rendered help the verb help paths read: the record's own
//!   text, then the options block with the flag descriptions, defaults, and
//!   shared globals
//!
//! The registry records in `cli_command_registry` carry every command's usage
//! text; a command with no record has no usage line here, and `usage_line`
//! returns `None` for it.
//!
//! ## Does not own
//! - The command → MCP tool bindings or the `help --json` catalog (see `cli_help`)
//! - Argument parsing (see `cli_args`) or help rendering (see `cli_output`)
//! - The catalog's description text (see `command_catalog`)
//! - The reserved surfaces' own rows (see `cli_help_surfaces`)
//!
//! ## Boundary
//!
//! Changes to a command's usage line should normally require changes to its
//! registry record, not here. Changes to a tool's schema should not require
//! changes here.
//!
//! Keep this module focused on the CLI usage strings.

/// The full usage text for a verb: its registry record's `full_usage`.
///
/// Every registered command's `--help` renders through here, so the record is
/// the one place that text lives. The bytes stay the record's own text, which
/// the record tests pin; the verb help paths that also show flag descriptions
/// and defaults read [`help_for`].
pub(crate) fn usage_for(verb: &str) -> Option<String> {
    crate::cli_command_registry::find(verb).map(super::cli_command_spec::CommandSpec::full_usage)
}

/// The rendered help for a verb: the record's `full_usage`, then the record's
/// options block, so every help path that shows flag data shows the same data:
/// each documented flag's description and default, then the accepted globals
/// from their shared definitions.
pub(crate) fn help_for(verb: &str) -> Option<String> {
    crate::cli_command_registry::find(verb).map(|spec| {
        let mut text = spec.full_usage();
        let options = spec.options_help();
        if !options.is_empty() {
            text.push_str("\n\n");
            text.push_str(&options);
        }
        text
    })
}

/// The canonical usage line for any command, with no prose and no `usage: `
/// prefix, for the surfaces that render it on their own.
///
/// A registry record answers. Every command the catalog lists resolves here,
/// so the catalog and each command's own `--help` read the same string. When
/// the two were separate hand-written texts, a flag added to one was absent
/// from the other, and a belief write needed `--source-type` that the catalog
/// never showed.
///
/// A command with no record returns `None`, which the catalog renders as an
/// explicit "no usage line registered" marker rather than as a silent omission.
pub(crate) fn usage_line(command: &str) -> Option<String> {
    crate::cli_command_registry::find(command).map(super::cli_command_spec::CommandSpec::usage_line)
}

#[cfg(test)]
#[path = "cli_help_usage_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_help_usage_render_tests.rs"]
mod render_tests;
