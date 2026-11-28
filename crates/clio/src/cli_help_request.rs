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
//! - `help_requested`, the one scan every surface reads its raw arguments with
//! - `is_value_flag` and `wants_help`, the arity-aware reading of a flag's
//!   value, which is the rule the flag parsers themselves refuse a missing
//!   value by
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

/// Whether `arg` is a flag whose following token is its value, read by arity
/// rather than by name alone: a declared global value flag, or any name the
/// catalog needs a value for wherever a parser accepts it.
///
/// It is the predicate `parse_flags` refuses a missing value with, so the help
/// scan and the parser cannot disagree about which token a flag claimed. A
/// valueless flag — a toggle such as `--insecure-no-auth`, or a value-optional
/// one such as `--bind` — never claims the token after it, so a help spelling
/// there stays a help request.
pub(crate) fn is_value_flag(arg: &str) -> bool {
    if crate::cli_args::is_global_value_flag(arg) {
        return true;
    }
    crate::cli_command_registry::requires_a_value(arg.trim_start_matches('-'))
}

/// Whether `args` ask for help on any surface: the same scan, reading each
/// flag's arity through [`is_value_flag`].
pub(crate) fn wants_help(args: &[String]) -> bool {
    help_requested(args, &is_value_flag)
}

#[cfg(test)]
#[path = "cli_help_request_tests.rs"]
mod tests;
