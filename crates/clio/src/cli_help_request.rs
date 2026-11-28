// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading a help request from a surface's raw
//! arguments before the `--` delimiter: the `help`, `--help`, and `-h` spellings,
//! read without taking a value flag's value; `--db help` names a database `help`.
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
/// read. The token after one is that flag's value when `cli_arg_token::is_flag_token`
/// says it is not itself a flag, the same rule `cli_args::parse` and
/// `parse_flags` apply: a lone `-` is a value, `-h` is not. A value flag's value
/// is never a help request, so `--db help` names a database called `help`.
///
/// A value flag with no value to its right is a missing value, and every parser
/// refuses that argv before any help screen is reached, so a help spelling
/// behind it is not a request either: `--db -h` and `--db --help` both report
/// the missing value. Tokens at or after a `--` delimiter are positional words
/// and are not read.
pub(crate) fn help_requested(args: &[String], is_value_flag: &impl Fn(&str) -> bool) -> bool {
    let args = crate::cli_args::before_delimiter(args);
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if is_value_flag(arg) {
            match args.get(index + 1) {
                Some(value) if !crate::cli_arg_token::is_flag_token(value) => {
                    index += 2;
                    continue;
                }
                _ => return false,
            }
        }
        if matches!(arg, "help" | "--help" | "-h") {
            return true;
        }
        index += 1;
    }
    false
}

/// Whether `args` ask for help on a surface that accepts `known` flags.
///
/// The surface's own value flags are read the same way the globals are, so a
/// surface flag's value is no more a help request than a global one's: `status
/// --dir DIR -h` still asks, while `status --dir -h` is the missing value the
/// parser reports. One rule for every surface, so a spaced value never shifts
/// where help is read from.
pub(crate) fn surface_help_requested(args: &[String], known: &[&str]) -> bool {
    help_requested(args, &|arg| {
        crate::cli_args::is_global_value_flag(arg) || known.contains(&arg.trim_start_matches('-'))
    })
}

#[cfg(test)]
#[path = "cli_help_request_tests.rs"]
mod tests;
