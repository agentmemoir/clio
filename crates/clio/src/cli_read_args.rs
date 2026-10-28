// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the argument helpers every read command
//! group shares: the bank/actor base arguments, positional handling, and the
//! numeric and enum flag checks.
//!
//! ## Owns
//! - `base_args` / `read_args`, the common read-tool arguments
//! - Positional-argument handling and its one-line usage errors
//! - `parse_u64`, `parse_f64`, and `validate_choice` flag validation
//! - `page_limit`, the shared `--limit` page-size contract, with its named
//!   default and maximum constants
//! - `require_flags`, the all-at-once check for a command's required flags
//! - The indefinite article used in a positional's usage hint
//!
//! ## Does not own
//! - Which flags a command group accepts (the command records own the accepted
//!   set; a group's `spec` answers from the registry)
//! - Command resolution and dispatch (see `cli_read`)
//! - Output rendering and error formatting (see `cli_output` / `cli_error`)
//!
//! ## Boundary
//!
//! Changes to the shared read-command argument contract should normally require
//! changes here. Changes to one command group's verbs or output should not
//! require changes here.
//!
//! Keep this module focused on reading and checking arguments, with no
//! dispatch and no I/O.

use crate::cli_args::ParsedArgs;
use crate::cli_error::CliError;
use serde_json::{Map, Value};

/// Bank and actor arguments common to every read tool.
pub(crate) fn base_args(parsed: &ParsedArgs) -> Map<String, Value> {
    let mut args = Map::new();
    if let Some(bank) = parsed.value("bank") {
        args.insert("bank".into(), Value::from(bank));
    }
    if let Some(actor) = parsed.value("actor") {
        args.insert("actor".into(), Value::from(actor));
    }
    args
}

/// Bank, actor, and the optional `explain` trace flag.
pub(crate) fn read_args(parsed: &ParsedArgs) -> Map<String, Value> {
    let mut args = base_args(parsed);
    if parsed.has("explain") {
        args.insert("explain".into(), Value::Bool(true));
    }
    args
}

pub(crate) fn one_positional<'a>(
    parsed: &'a ParsedArgs,
    verb: &str,
    label: &str,
) -> Result<&'a str, CliError> {
    match parsed.positionals() {
        [only] => Ok(only.as_str()),
        [] => Err(CliError::usage_hint(
            format!("`{verb}` requires {} {label} argument", article(label)),
            format!("usage: clio {verb} {label}"),
        )),
        _ => Err(unexpected_extra(parsed, 1, Some(verb), label)),
    }
}

pub(crate) fn no_positionals(parsed: &ParsedArgs) -> Result<(), CliError> {
    match parsed.positionals().first() {
        Some(_) => Err(unexpected_extra(parsed, 0, None, "no positional arguments")),
        None => Ok(()),
    }
}

/// The exit-2 error for a positional the command does not take.
///
/// The token in front of the extra value decides the hint. When it is a boolean
/// flag, the value was almost certainly meant for that flag: `--dry-run false` is
/// the shape of that mistake, and "flag `--dry-run` takes no value" names the fix
/// where the bare "unexpected extra argument" leaves the reader guessing. Any
/// other time the hint states what the command does accept, so the caller does
/// not have to run `--help` to learn it.
pub(crate) fn unexpected_extra(
    parsed: &ParsedArgs,
    index: usize,
    verb: Option<&str>,
    accepts: &str,
) -> CliError {
    let extra = parsed.positional(index).unwrap_or_default();
    let usage = match verb {
        Some(verb) => format!("usage: clio {verb} {accepts}"),
        None => format!("this command takes {accepts}"),
    };
    let hint = match parsed.bool_before(index) {
        Some(flag) => format!("flag `--{flag}` takes no value; pass `--{flag}` on its own"),
        None => usage,
    };
    CliError::usage_hint(format!("unexpected extra argument `{extra}`"), hint)
}

/// The required flags of every verb whose record has not arrived yet, by the
/// name a reader would type.
///
/// A verb whose command record has arrived answers from the record instead (see
/// [`required_flags`]), so its required set and the usage line that command
/// prints come from the same data and cannot disagree. Every verb this phase
/// moved has a record, so the table is empty; an unregistered verb still
/// enforces its mandated flags in its own build, and a row is added here only
/// when that verb routes through [`require_flags`]. A new mandated flag on such
/// a verb must add a row here, and the test beside it derives the row from the
/// usage line and fails when the two differ.
pub(crate) const REQUIRED_FLAGS: &[(&str, &[&str])] = &[];

/// A verb's required flags: the registry record's when the verb is registered,
/// else the table row.
///
/// A registered record owns its command's flags, so its required set cannot
/// drift from the usage line that command prints. The table stays the answer for
/// every verb without a record.
pub(crate) fn required_flags(verb: &str) -> Vec<&'static str> {
    if let Some(spec) = crate::cli_command_registry::find(verb) {
        return spec.required_flags();
    }
    REQUIRED_FLAGS
        .iter()
        .find(|(name, _)| *name == verb)
        .map_or_else(Vec::new, |(_, flags)| flags.to_vec())
}

/// A flag in a usage line that is written bare rather than inside brackets, and so
/// is one the verb cannot run without.
///
/// The brackets are followed, not the text: `[--a X|--b Y]` is one optional group
/// whose members are alternatives, and `[--flag]...` is one optional repeatable. A
/// flag is required only when no bracket is open around it.
///
/// A top-level `|` separates whole command forms (`clio backup restore ID |
/// clio backup restore --latest`): the scan reads every form rather than
/// stopping at the alternation, so a flag that only one form writes bare still
/// reads as bare. Such a flag is not required, because the verb also runs in
/// the other form; the bare-flag parity test excuses it by name, with a row for
/// `backup restore` and `backup verify`, so a new alternation fails that test
/// until the flag has a row instead of being excused silently.
#[cfg(test)]
pub(crate) fn bare_flags(usage: &str) -> Vec<String> {
    let mut depth = 0usize;
    let mut bare = Vec::new();
    for token in usage.split(' ') {
        for part in token.chars() {
            match part {
                '[' => depth += 1,
                ']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if depth == 0
            && let Some(flag) = token.strip_prefix("--")
        {
            let end = flag
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .unwrap_or(flag.len());
            bare.push(format!("--{}", &flag[..end]));
        }
    }
    bare
}

/// Check that every required flag of one command is present, all at once.
///
/// # Errors
/// A usage error (exit 2) naming every missing flag in the first run. Reporting
/// them one at a time made `clio failure record T` fail three times before it
/// accepted the command, which reads as three separate mistakes rather than one.
pub(crate) fn require_flags(
    parsed: &ParsedArgs,
    verb: &str,
    flags: impl AsRef<[&'static str]>,
) -> Result<(), CliError> {
    let flags = flags.as_ref();
    let missing: Vec<String> = flags
        .iter()
        .copied()
        .filter(|flag| parsed.value(flag).is_none())
        .map(|flag| format!("--{flag}"))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let count = if missing.len() == 1 {
        String::new()
    } else {
        format!(" ({} missing)", missing.len())
    };
    let example = flags
        .iter()
        .map(|flag| format!("--{flag} VALUE"))
        .collect::<Vec<_>>()
        .join(" ");
    Err(CliError::usage_hint(
        format!("`{verb}` requires {}{count}", missing.join(", ")),
        format!("try `clio {verb} ... {example}`"),
    ))
}

/// A flag [`require_flags`] has already proved is present.
///
/// The list in [`require_flags`] is the one place that decides what a command
/// requires. This accessor only reads a value that list has already checked, so
/// it never asks the caller whether the flag is there a second time.
///
/// An absent flag reads as an empty string rather than panicking. A caller that
/// forgets the check therefore sends an empty value to the tool, which refuses it
/// with its own message; the failure stays an error the user can act on.
pub(crate) fn flag<'a>(parsed: &'a ParsedArgs, name: &str) -> &'a str {
    parsed.value(name).unwrap_or_default()
}

pub(crate) fn parse_u64(flag: &str, raw: &str) -> Result<u64, CliError> {
    raw.parse::<u64>().map_err(|_| {
        CliError::usage_hint(
            format!("--{flag} must be a non-negative integer, got `{raw}`"),
            format!("try `--{flag} 10`"),
        )
    })
}

/// The page size a page-capable read command serves when `--limit` is omitted.
pub(crate) const DEFAULT_PAGE_LIMIT: u64 = 50;
/// The largest page size the CLI serves for `--limit`; a larger value is
/// clamped to this maximum rather than refused.
pub(crate) const MAX_PAGE_LIMIT: u64 = 200;

/// The page size a command serves for the caller's `--limit` value.
///
/// One shared rule, so every page-capable command answers a given `--limit`
/// the same way. Omitted reads as [`DEFAULT_PAGE_LIMIT`], and `0` is refused:
/// a page of nothing is a mistake, not a request. Anything above
/// [`MAX_PAGE_LIMIT`] is clamped to the maximum instead of refused — a caller
/// asking for more than the cap still gets the largest page the command
/// serves — and the lowering is disclosed in the command's output by
/// `add_requested_limit`, so the served page never silently disagrees with
/// the request.
///
/// # Errors
/// A usage error when the parsed limit is `0`.
pub(crate) fn page_limit(flag: &str, requested: Option<u64>) -> Result<u64, CliError> {
    match requested {
        None => Ok(DEFAULT_PAGE_LIMIT),
        Some(0) => Err(CliError::usage_hint(
            format!("--{flag} must be a positive page size, got `0`"),
            format!("try `--{flag} 10`"),
        )),
        Some(value) => Ok(value.min(MAX_PAGE_LIMIT)),
    }
}

/// Parse a floating-point flag value, reporting the flag the caller typed.
pub(crate) fn parse_f64(flag: &str, raw: &str) -> Result<f64, CliError> {
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            CliError::usage_hint(
                format!("--{flag} must be a finite number, got `{raw}`"),
                format!("try `--{flag} 0.5`"),
            )
        })
}

/// Parse a floating-point flag that must lie in `0.0..=1.0`.
///
/// A value that parses but is out of range is a usage error, like every other
/// caller-supplied flag mistake, rather than an `out_of_range` from the domain
/// layer. The unit-interval flags (`--confidence`, and the floors in `cli_read`)
/// already bound their values at the boundary.
pub(crate) fn parse_unit_f64(flag: &str, raw: &str) -> Result<f64, CliError> {
    let value = parse_f64(flag, raw)?;
    if (0.0..=1.0).contains(&value) {
        return Ok(value);
    }
    Err(CliError::usage_hint(
        format!("--{flag} must be between 0.0 and 1.0, got `{raw}`"),
        format!("try `--{flag} 0.5`"),
    ))
}

/// Validate that a flag value is one of `allowed`, fail closed otherwise.
///
/// The hint names the value closest to what was typed, and lists every value when
/// nothing is close. Offering `allowed[0]` unconditionally sent each caller to
/// the same value however wrong it was for them.
pub(crate) fn validate_choice<'a>(
    flag: &str,
    raw: &'a str,
    allowed: &[&str],
) -> Result<&'a str, CliError> {
    if allowed.contains(&raw) {
        return Ok(raw);
    }
    let hint = match crate::cli_args::suggest(raw, allowed) {
        Some(closest) => format!("did you mean `--{flag} {closest}`?"),
        None => format!("--{flag} takes one of: {}", allowed.join(", ")),
    };
    Err(CliError::usage(format!(
        "--{flag} must be {}, got `{raw}`",
        allowed.join("|")
    ))
    .with_hint(hint))
}

/// Validate the bi-temporal `--time-axis` value.
pub(crate) fn validate_time_axis(raw: &str) -> Result<&str, CliError> {
    validate_choice("time-axis", raw, &["valid", "transaction"])
}

/// The indefinite article for a positional label (`an ID`, `a QUERY`).
pub(crate) fn article(label: &str) -> &'static str {
    match label.chars().next() {
        Some('A' | 'E' | 'I' | 'O' | 'U' | 'a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    }
}

#[cfg(test)]
#[path = "cli_read_args_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_read_args_spec_tests.rs"]
mod spec_tests;

#[cfg(test)]
#[path = "cli_read_args_page_limit_tests.rs"]
mod page_limit_tests;
