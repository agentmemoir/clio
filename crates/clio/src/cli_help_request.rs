// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading a help request from a surface's raw
//! arguments: the `help`, `--help`, and `-h` spellings, read without taking a
//! value flag's value as a request. `--db help` names a database called `help`.
//!
//! ## Owns
//! - `help_requested`, the scan shared by the top level and the reserved
//!   surfaces
//!
//! ## Does not own
//! - The help screen a surface prints (`cli_top_level`, `cli_help`)
//! - The flag grammar that decides which tokens are values (`cli_args`)
//!
//! ## Boundary
//!
//! Changes to which spellings mean help should normally require changes here.
//!
//! Keep this module focused on reading a help request from raw arguments.

/// Whether `args` ask for help, skipping the value of a value flag.
///
/// `is_value_flag` answers whether a token is a value flag on the surface being
/// read; the token after one is that flag's value when it does not itself start
/// with `--`, the rule `cli_args::parse` and `parse_flags` already apply. A
/// value flag's value is not a help request: `--db help` names a database
/// called `help`.
pub(crate) fn help_requested(args: &[String], is_value_flag: &impl Fn(&str) -> bool) -> bool {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if is_value_flag(arg)
            && args
                .get(index + 1)
                .is_some_and(|value| !value.starts_with("--"))
        {
            index += 2;
            continue;
        }
        if matches!(arg, "help" | "--help" | "-h") {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
#[path = "cli_help_request_tests.rs"]
mod tests;
