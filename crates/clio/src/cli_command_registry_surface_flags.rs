// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the accepted-flag names the reserved
//! surfaces' parsers gate on: the derivation from each surface's record, and
//! the hidden flags a usage line never prints.
//!
//! ## Owns
//! - `parse_flag_names`, the name list `parse_flags` gates a reserved surface
//!   on
//! - `hidden_accepted_flags`, the flags a surface accepts without printing,
//!   `--shared-bank` on the two transports
//!
//! ## Does not own
//! - The argument grammar (`parse_flags`, see `cli_top_level` and `cli_args`)
//! - The records the names derive from (see `cli_command_registry_surface`)
//! - The surfaces' own dispatch and their bespoke parsers (see their `*_cli`
//!   modules; `setup` parses through its record's `Flag` entries)
//!
//! ## Boundary
//!
//! Changes to which flags a reserved surface accepts should normally require
//! changes to its record, not here. Changes to how the names are derived
//! should require changes here.
//!
//! Keep this module focused on the accepted-name derivation.

/// `--shared-bank` opts a deployment into the shared bank at open time. Both
/// transports accept it and neither usage line prints it, so the records carry
/// it here instead of as a documented flag.
pub(crate) const SHARED_BANK: &str = "shared-bank";

/// The flags a reserved surface accepts that its usage line never prints, by
/// record name.
pub(crate) fn hidden_accepted_flags(command: &str) -> &'static [&'static str] {
    match command {
        "mcp stdio" | "mcp http" => &[SHARED_BANK],
        _ => &[],
    }
}

/// The flag names a reserved surface's parser accepts: its record's documented
/// names plus its hidden accepted ones.
///
/// The list is alphabetical. The unknown-flag suggestion compares candidates in
/// order and keeps the earlier one on a tie, and only candidates sharing the
/// typo's first letter ever compete, so one alphabetical order preserves every
/// near-miss answer the per-surface hand tables this derivation replaced gave.
///
/// A name here gates `parse_flags`, which reads names alone; each flag's arity
/// lives on the record's `Flag` entries.
pub(crate) fn parse_flag_names(command: &str) -> Vec<&'static str> {
    let spec = match command {
        "compose up" => &super::COMPOSE_UP,
        "compose down" => &super::COMPOSE_DOWN,
        other => crate::cli_command_registry::find(other)
            .unwrap_or_else(|| panic!("`{other}` is a registered command")),
    };
    let mut names: Vec<&'static str> = spec.documented.iter().map(|flag| flag.long).collect();
    names.extend(hidden_accepted_flags(command).iter().copied());
    names.sort_unstable();
    names
}

#[cfg(test)]
#[path = "cli_command_registry_surface_flags_tests.rs"]
mod tests;
