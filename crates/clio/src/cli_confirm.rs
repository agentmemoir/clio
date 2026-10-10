// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared destructive-confirmation gate the
//! CLI mutating commands use before they may change stored memory.
//!
//! ## Owns
//! - The confirm-gate decision (explicit `--confirm`, the global `--yes`, or a
//!   `--dry-run` preview) and the exit-2 refusal that names the missing flag
//!
//! ## Does not own
//! - Flag parsing (see `cli_args`) or any command's flags/semantics (see the
//!   `cli_write_*` modules)
//! - Exit-code values (see `cli_error`) or tool-level confirmation inside
//!   `clio-mcp`
//!
//! ## Boundary
//!
//! Changes to how a destructive CLI command is confirmed should normally require
//! changes here. Changes to a specific command's behavior should not.
//!
//! Keep this module focused on the destructive-confirmation decision.

use crate::cli_args::ParsedArgs;
use crate::cli_error::CliError;

/// True when the caller already consented to a destructive change.
///
/// `--confirm` is the per-command flag; the global `--yes` is the non-interactive
/// alias the rest of the CLI already accepts.
pub(crate) fn confirmed(parsed: &ParsedArgs) -> bool {
    parsed.has("confirm") || parsed.has("yes")
}

/// Refuse a destructive command unless it was confirmed or only previewed.
///
/// The gate is deliberately flag-based and TTY-independent: the CLI adds no
/// interactive prompt, so a destructive command with neither `--confirm`
/// (or `--yes`) nor `--dry-run` fails closed with exit 2 and names the missing
/// flag. `--dry-run` passes because it performs zero writes.
///
/// # Errors
/// A usage error (exit 2) naming `--confirm` when the change was not confirmed.
pub(crate) fn require_confirm(verb: &str, parsed: &ParsedArgs) -> Result<(), CliError> {
    if parsed.has("dry-run") || confirmed(parsed) {
        return Ok(());
    }
    Err(CliError::usage_hint(
        format!("`{verb}` is destructive and was refused without confirmation"),
        format!(
            "pass --confirm (or --yes) to run `clio {verb} ...`; use --dry-run to preview first"
        ),
    ))
}

#[cfg(test)]
#[path = "cli_confirm_tests.rs"]
mod tests;
