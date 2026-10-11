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
//! - `require_flags`, the all-at-once check for a command's required flags
//! - The indefinite article used in a positional's usage hint
//!
//! ## Does not own
//! - Which flags a command group accepts (each group owns its `FlagSpec`)
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

/// Check that every required flag of one command is present, all at once.
///
/// # Errors
/// A usage error (exit 2) naming every missing flag in the first run. Reporting
/// them one at a time made `clio failure record T` fail three times before it
/// accepted the command, which reads as three separate mistakes rather than one.
/// Which flags a verb cannot run without, by the name a reader would type.
///
/// The usage registry already says which flags are required: a required flag is
/// written bare and an optional one inside brackets. This table repeats that, so it
/// could drift from the help a reader sees — which is the same defect the catalog
/// had, where a verb's own `--help` and `clio help` disagreed. The test beside this
/// table derives the required set from the usage line and fails when the two differ,
/// so the drift is caught rather than trusted.
///
/// The three entries that differ from a bare reading each carry their reason below.
pub(crate) const REQUIRED_FLAGS: &[(&str, &[&str])] = &[
    ("update", &["rule"]),
    ("discard", &["reason"]),
    ("correct", &["reason"]),
    ("triple end", &["valid-until"]),
    ("belief observe", &["confidence", "evidence", "source"]),
    ("graph link", &["relationship"]),
    ("task upsert", &["definition", "status"]),
    ("failure record", &["attempt-n", "what-failed", "lesson"]),
    ("export", &["out"]),
    ("import", &["source"]),
    ("erase", &["subject", "basis", "request"]),
    ("canonical put", &["category", "epistemic-kind"]),
];

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
#[cfg(test)]
pub(crate) fn bare_flags(usage: &str) -> Vec<String> {
    let mut depth = 0usize;
    let mut bare = Vec::new();
    for token in usage.split(' ') {
        for part in token.chars() {
            match part {
                '[' | '|' if depth == 0 && part == '[' => depth += 1,
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
