// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one rule both CLI parsers apply to a raw
//! token before either reads it: whether the token is a flag rather than a
//! value, and what a surface says when the token it needs is not a value.
//!
//! ## Owns
//! - `is_flag_token`, the predicate every value consumer calls: dash-led, but
//!   not the lone `-`, which is a real value such as stdin for `--file`
//! - `out_value`, taking `--out`'s value token, refused when it is absent or
//!   dash-led, so `--out -h` is never read as a directory named `-h`
//! - `unexpected_argument`, the refusal for a token with no dash on a surface
//!   that has no operand slot to hand it to
//!
//! ## Does not own
//! - The grammar that consumes the tokens (see `cli_args`) and the reserved
//!   surfaces' flag map that applies the same rule (see `cli_top_level`)
//! - The error type, and the refusals that are not about a token (see
//!   `cli_error`)
//!
//! ## Boundary
//!
//! A change to what counts as a flag, or to how a value flag's value token is
//! taken, belongs here. A change to a flag set, a verb, or a message that is
//! not about a token does not.
//!
//! It is its own file because both parsers need the rule and neither owns it:
//! while the reserved surfaces decided for themselves whether the token after a
//! value flag was a value, the two parsers answered differently for the same
//! input.

use crate::cli_error::CliError;

/// Whether `token` is a flag the grammar must not read as a value: dash-led,
/// but not the lone `-`, which is a real value such as stdin for `--file`.
pub(crate) fn is_flag_token(token: &str) -> bool {
    token.starts_with('-') && token != "-"
}

/// The value token after a bare `--out`: refused when it is absent or starts
/// with `-`, so `--out -h` is a missing value, never a directory named `-h`;
/// `--out=DIR` keeps its token as typed. Blank values are refused too.
pub(crate) fn out_value(args: &[String], index: usize) -> Result<String, CliError> {
    match args.get(index + 1) {
        Some(value) if !value.starts_with('-') => {
            crate::cli_error::reject_blank_value("--out", value)?;
            Ok(value.clone())
        }
        _ => Err(CliError::usage_hint(
            "--out requires a directory value",
            "pass the directory after --out, or spell it `--out=DIR`",
        )),
    }
}

/// A token with no leading dash, which these surfaces have no operand slot for.
/// It is refused so a mistyped flag cannot arrive as a silent no-op.
pub(crate) fn unexpected_argument(token: &str) -> CliError {
    CliError::usage(format!("unexpected argument `{token}`"))
        .with_hint("these commands take flags only; run `clio help` for the list")
}

#[cfg(test)]
#[path = "cli_arg_token_tests.rs"]
mod tests;
