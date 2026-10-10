// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hand-rolled CLI argument-parser contract:
//! positionals, the `--` end-of-options delimiter, value flags
//! (`--flag value` / `--flag=value`), boolean flags, global flags before or
//! after the verb, and fail-closed unknown flags.
//!
//! ## Owns
//! - `FlagSpec` (global + command flags) and `ParsedArgs` (positionals + flags)
//! - The `parse` grammar and its `did you mean` suggestions
//! - Splitting leading global flags from the verb
//!
//! ## Does not own
//! - Command semantics (see `cli_read`) or output rendering (see `cli_output`)
//! - The error type (see `cli_error`)
//!
//! ## Boundary
//!
//! Changes to the parser grammar should normally require changes here.
//! Changes to a command's flag set should not require changes here.
//!
//! The top-level grammar is `GLOBAL_FLAG* VERB COMMAND_ARG*`. A command-argument
//! list may contain one exact `--`; that token ends option parsing and every
//! later token is positional. A top-level `--` is not a command selector, so
//! `clio -- remember` is intentionally unsupported. Keep this module focused
//! on the CLI argument grammar.

use crate::cli_error::CliError;
use std::collections::{HashMap, HashSet};

/// Value-taking global flags, accepted before or after the verb.
pub(crate) const GLOBAL_VALUE_FLAGS: &[&str] = &["db", "backend", "bank", "actor", "output"];
/// Boolean global flags (never consume a following token).
pub(crate) const GLOBAL_BOOL_FLAGS: &[&str] = &["no-input", "yes"];

/// The flags a command accepts: its own plus the globals.
pub(crate) struct FlagSpec {
    value: Vec<&'static str>,
    boolean: Vec<&'static str>,
    /// Value flags that may repeat, each occurrence kept in order.
    repeat: Vec<&'static str>,
}

impl FlagSpec {
    /// Build a spec from a command's value flags and boolean flags; globals are
    /// merged in, de-duplicated.
    pub(crate) fn new(value: &'static [&'static str], boolean: &'static [&'static str]) -> Self {
        let mut all_value = GLOBAL_VALUE_FLAGS.to_vec();
        for flag in value {
            if !all_value.contains(flag) {
                all_value.push(flag);
            }
        }
        Self {
            value: all_value,
            boolean: {
                let mut all_boolean = GLOBAL_BOOL_FLAGS.to_vec();
                for flag in boolean {
                    if !all_boolean.contains(flag) {
                        all_boolean.push(flag);
                    }
                }
                all_boolean
            },
            repeat: Vec::new(),
        }
    }

    /// Mark value flags as repeatable: every occurrence is collected in order
    /// and read back with `ParsedArgs::values_of` instead of the last one
    /// winning. A repeatable flag is parsed as a value flag by every other
    /// rule (unknown-flag checks, missing-value errors).
    pub(crate) fn repeating(mut self, flags: &'static [&'static str]) -> Self {
        self.repeat.extend_from_slice(flags);
        self
    }

    /// Value-taking flag names, globals included.
    pub(crate) fn value_flags(&self) -> &[&'static str] {
        &self.value
    }

    /// Boolean flag names, globals included.
    pub(crate) fn bool_flags(&self) -> &[&'static str] {
        &self.boolean
    }

    /// Whether a value flag may repeat.
    fn repeats(&self, name: &str) -> bool {
        self.repeat.contains(&name)
    }
}

/// Parsed arguments: ordered positionals, value flags, and boolean flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedArgs {
    positionals: Vec<String>,
    values: HashMap<String, String>,
    /// Collected occurrences of repeatable value flags, in command-line order.
    multi: HashMap<String, Vec<String>>,
    flags: HashSet<String>,
    /// One entry per positional, aligned by index: the boolean flag token that
    /// immediately preceded it, if any.
    ///
    /// `--dry-run false` parses `false` as a positional because `--dry-run` is a
    /// boolean flag and takes no value. Without this the "unexpected extra
    /// argument" error cannot say that the value belongs to the flag in front of
    /// it, which is the whole explanation the user needs.
    bool_before_positional: Vec<Option<String>>,
}

impl ParsedArgs {
    /// The positional at `index`, when present.
    pub(crate) fn positional(&self, index: usize) -> Option<&str> {
        self.positionals.get(index).map(String::as_str)
    }

    /// Every positional, in order.
    pub(crate) fn positionals(&self) -> &[String] {
        &self.positionals
    }

    /// The value of a value-taking flag, when given.
    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Every collected occurrence of a repeatable value flag, in order.
    pub(crate) fn values_of(&self, name: &str) -> &[String] {
        self.multi.get(name).map_or(&[], Vec::as_slice)
    }

    /// Whether a boolean flag (or `--help`) was given.
    pub(crate) fn has(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    /// The boolean flag token immediately before positional `index`, if any.
    ///
    /// `None` for an index that carries no positional, so a caller can pass the
    /// index it is about to complain about without checking the length first.
    pub(crate) fn bool_before(&self, index: usize) -> Option<&str> {
        self.bool_before_positional
            .get(index)
            .and_then(|flag| flag.as_deref())
    }
}

/// Parse `args` (everything after the verb) against `spec`.
///
/// # Errors
/// A usage error (exit 2) for an unknown flag, a missing flag value, a value
/// supplied to a boolean flag, or an empty value supplied to a value flag.
/// Unknown flags fail closed with a suggestion.
///
/// Every value flag follows one rule for an empty value: it is refused, and the
/// message names the flag. The alternative — dropping an empty value and running
/// the command as if the flag were absent — is what made `--as-of ''` quietly
/// search every memory instead of the one the caller asked for. A caller that
/// wants the unfiltered behaviour omits the flag.
///
/// # Panics
/// Never. The grammar is total over `String` tokens.
pub(crate) fn parse(
    spec: &FlagSpec,
    args: impl IntoIterator<Item = String>,
) -> Result<ParsedArgs, CliError> {
    let tokens: Vec<String> = args.into_iter().collect();
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    // The boolean flag token the next positional would be blamed on, cleared by
    // anything that consumes a token of its own.
    let mut last_bool: Option<String> = None;
    while index < tokens.len() {
        if tokens[index] == "--" {
            // Everything after the delimiter is a positional. The first one still
            // follows whatever token came before the delimiter, so the alignment
            // with `bool_before_positional` is kept.
            for (offset, token) in tokens[index + 1..].iter().enumerate() {
                parsed.positionals.push(token.clone());
                parsed
                    .bool_before_positional
                    .push((offset == 0).then(|| last_bool.take()).flatten());
            }
            break;
        }
        let token = tokens[index].clone();
        index += 1;
        if token == "-h" || token == "--help" {
            parsed.flags.insert("help".to_owned());
            last_bool = None;
            continue;
        }
        // `-o` is the short spelling of the global `--output` value flag and
        // accepts the same space and `=` forms; attached `-ojson` stays
        // unknown so every other single-dash token still fails closed.
        if token == "-o" || token.starts_with("-o=") {
            let value = match token.strip_prefix("-o=") {
                Some(inline) => inline.to_owned(),
                None => match tokens.get(index) {
                    Some(next) if !next.starts_with("--") => {
                        index += 1;
                        next.clone()
                    }
                    _ => {
                        return Err(CliError::usage_hint(
                            "flag `-o` requires a value",
                            "try `-o json` or `-o text`",
                        ));
                    }
                },
            };
            crate::cli_error::reject_empty_value("-o", &value)?;
            parsed.values.insert("output".to_owned(), value);
            last_bool = None;
            continue;
        }
        let Some(body) = token.strip_prefix("--") else {
            if token.starts_with('-') && token != "-" {
                return Err(unknown_flag(&token, spec));
            }
            parsed.positionals.push(token);
            parsed.bool_before_positional.push(last_bool.take());
            continue;
        };
        let (name, inline) = match body.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (body, None),
        };
        if spec.bool_flags().contains(&name) {
            if inline.is_some() {
                return Err(CliError::usage(format!("flag `--{name}` takes no value")));
            }
            parsed.flags.insert(name.to_owned());
            last_bool = Some(name.to_owned());
            continue;
        }
        if spec.value_flags().contains(&name) {
            let value = if let Some(value) = inline {
                value
            } else {
                match tokens.get(index) {
                    Some(next) if !next.starts_with("--") => {
                        index += 1;
                        next.clone()
                    }
                    _ => {
                        return Err(CliError::usage_hint(
                            format!("flag `--{name}` requires a value"),
                            format!("try `--{name} VALUE`"),
                        ));
                    }
                }
            };
            crate::cli_error::reject_empty_value(&format!("--{name}"), &value)?;
            if spec.repeats(name) {
                parsed.multi.entry(name.to_owned()).or_default().push(value);
            } else {
                parsed.values.insert(name.to_owned(), value);
            }
            last_bool = None;
            continue;
        }
        return Err(unknown_flag(&token, spec));
    }
    Ok(parsed)
}

/// Split leading global flags from the verb so they may appear before it.
///
/// Returns `(rest, globals)`: `rest` starts at the first non-global token (the
/// verb and its arguments); `globals` carries the leading global flags in order
/// so the caller can append them after the verb's own arguments.
pub(crate) fn split_leading_globals(args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut globals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            break;
        }
        let (name, inline) = if let Some(body) = token.strip_prefix("--") {
            match body.split_once('=') {
                Some((name, _)) => (name, true),
                None => (body, false),
            }
        } else if token == "-o" {
            ("output", false)
        } else if token.starts_with("-o=") {
            ("output", true)
        } else {
            break;
        };
        if GLOBAL_BOOL_FLAGS.contains(&name) && !inline {
            globals.push(token.clone());
            index += 1;
            continue;
        }
        if GLOBAL_VALUE_FLAGS.contains(&name) {
            globals.push(token.clone());
            index += 1;
            // A following flag token is not a value: leave it for the next
            // iteration so `--output --db X` reports `--output` as missing its
            // value instead of swallowing `--db`.
            if !inline
                && let Some(value) = args.get(index)
                && !value.starts_with("--")
            {
                globals.push(value.clone());
                index += 1;
            }
            continue;
        }
        break;
    }
    (args[index..].to_vec(), globals)
}

/// Put leading globals before the command's first `--` when one exists.
///
/// Without a delimiter, preserve the existing order by appending globals after
/// the command arguments. This keeps configuration flags parseable when a
/// positional value ends command option parsing.
pub(crate) fn merge_leading_globals(
    command_args: impl IntoIterator<Item = String>,
    globals: Vec<String>,
) -> Vec<String> {
    let mut command_args: Vec<String> = command_args.into_iter().collect();
    if let Some(delimiter) = command_args.iter().position(|arg| arg == "--") {
        command_args.splice(delimiter..delimiter, globals);
    } else {
        command_args.extend(globals);
    }
    command_args
}

/// Build the fail-closed unknown-flag error, suggesting the closest known flag.
fn unknown_flag(token: &str, spec: &FlagSpec) -> CliError {
    let name = token.trim_start_matches('-');
    let error = CliError::usage(format!("unknown flag `{token}`"));
    // The removed `--sync-key` flag has no close neighbour to suggest, so name
    // the current key route instead: the sync usage lines and the MCP surface
    // both read the transfer key from the recorded store or `CLIO_SYNC_KEY_FILE`.
    if name == "sync-key" {
        return error.with_hint(
            "the transfer key is read from this node's sync key store (run `clio sync keygen`), \
             or the file CLIO_SYNC_KEY_FILE names",
        );
    }
    let mut candidates: Vec<&str> = spec
        .value_flags()
        .iter()
        .chain(spec.bool_flags())
        .copied()
        .collect();
    candidates.push("help");
    let hint = suggest(name, &candidates).map(|best| format!("did you mean `--{best}`?"));
    match hint {
        Some(hint) => error.with_hint(hint),
        None => error,
    }
}

#[cfg(test)]
#[path = "cli_args_tests.rs"]
mod tests;

/// The matcher lives in its own module and its own file; re-exported here so the
/// grammar and its callers keep one import path for it.
pub(crate) use crate::cli_args_suggest::suggest;
