// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio ranking` CLI surface: showing the
//! effective ranking/admission environment and applying a validated weight
//! patch, with `--dry-run` returning the normalized weights without applying.
//!
//! ## Owns
//! - The `ranking get|set` verbs and their text views
//! - Decoding the `--patch` / `--patch-file` JSON object
//!
//! ## Does not own
//! - Ranking weight math or validation (`clio-config`)
//! - The verbs' static records and verb list (see
//!   `cli_command_registry_ranking_retention`)
//! - The runtime-backed command engine (see `runtime_cli`)
//!
//! ## Boundary
//!
//! Changes to the ranking CLI flags or views should normally require changes here.
//! Changes to ranking semantics should not require changes here.
//!
//! Keep this module focused on the `clio ranking` surface.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_output;
use crate::cli_read::CliIo;
use crate::runtime_cli::{RuntimeGroup, no_positionals, pretty_without_ok};
use clio::Runtime;
use serde_json::{Map, Value, json};
use std::process::ExitCode;

/// The `clio ranking` command group.
pub(crate) struct RankingGroup;

impl RuntimeGroup for RankingGroup {
    /// The group's verb keys: the registry records' own names, so the
    /// dispatcher has no second list to drift.
    fn verbs(&self) -> &'static [&'static str] {
        crate::cli_command_registry_ranking_retention::RANKING_VERBS
    }

    /// The flags `verb` accepts: the verb's registry record's parser spec,
    /// globals included.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(spec) => spec.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
        match verb {
            "ranking get" => {
                no_positionals(parsed)?;
                Ok(("ranking_env_get", json!({})))
            }
            _ => set_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value) -> String {
        match verb {
            "ranking get" => format!("ranking environment:\n{}", pretty_without_ok(payload)),
            _ => render_set(payload),
        }
    }
}

/// Dispatch `clio ranking <subcommand>`.
pub(crate) fn ranking(args: Vec<String>) -> ExitCode {
    let env = |key: &str| std::env::var(key).ok();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut input = std::io::stdin();
    let mut io = CliIo {
        out: &mut out,
        err: &mut err,
        stdin: &mut input,
        is_tty: cli_output::stdout_is_tty(),
        no_color: cli_output::no_color_requested(),
        env: &env,
    };
    crate::runtime_cli::run(&RankingGroup, "ranking", args, &mut io, &mut Runtime::new())
}

/// `ranking set --patch JSON [--patch-file FILE] [--dry-run]` → `ranking_env_set`.
///
/// The patch must be a JSON object; unknown fields and non-normalized weight
/// groups are rejected or renormalized by the tool, never by the CLI.
fn set_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    let (raw, flag) = if let Some(inline) = parsed.value("patch") {
        (inline.to_owned(), "--patch")
    } else if let Some(path) = parsed.value("patch-file") {
        let raw = std::fs::read_to_string(path).map_err(|error| {
            CliError::usage(format!("--patch-file `{path}` is not readable: {error}"))
        })?;
        (raw, "--patch-file")
    } else {
        return Err(CliError::usage_hint(
            "`ranking set` requires --patch (a JSON object) or --patch-file FILE",
            "try 'clio ranking set --patch '{\"theta_admit\":0.6}''",
        ));
    };
    let patch: Value = serde_json::from_str(&raw)
        .map_err(|error| CliError::usage(format!("{flag} must be a JSON object: {error}")))?;
    if !patch.is_object() {
        return Err(CliError::usage(format!("{flag} must be a JSON object")));
    }
    let mut args = Map::new();
    args.insert("patch".into(), patch);
    args.insert("dry_run".into(), Value::Bool(parsed.has("dry-run")));
    Ok(("ranking_env_set", Value::Object(args)))
}

/// Human text for `ranking set`: the mode plus the resulting normalized weights.
fn render_set(payload: &Value) -> String {
    let dry_run = payload
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let applied = payload
        .get("applied")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mode = if dry_run {
        "dry-run (not applied)"
    } else if applied {
        "applied to this session"
    } else {
        "not applied"
    };
    let weights = payload
        .get("ranking")
        .map_or_else(|| "{}".to_owned(), Value::to_string);
    let mut text = format!("ranking set {mode}: normalized weights\n{weights}");
    if applied && !dry_run {
        // A one-shot process cannot keep a session change; say so plainly.
        text.push_str("\nnote: a session ranking change lasts only for this process; use --dry-run to preview");
    }
    text
}

#[cfg(test)]
#[path = "ranking_cli_tests.rs"]
mod tests;
