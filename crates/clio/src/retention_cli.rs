// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio retention` CLI surface: getting, setting, and
//! printing the schema of a bank-scoped retention profile.
//!
//! ## Owns
//! - `clio retention get|set|schema` argument decoding and exit codes
//! - Retention store path selection from `--profiles` / `CLIO_RETENTION_PROFILES`
//!
//! ## Does not own
//! - The profile object, mapping numbers, or storage format (see `clio-config`)
//! - The subcommands' static records (see
//!   `cli_command_registry_ranking_retention`)
//! - Profile resolution semantics or admission scoring
//!
//! ## Boundary
//!
//! Changes to a retention verb's flags, usage text, or summary should normally
//! require changes in `cli_command_registry_ranking_retention`; changes to its
//! dispatch, decoding, or views should normally require changes here. Changes
//! to profile semantics should not require changes here.
//!
//! Keep this module focused on the retention CLI surface.

use crate::cli_top_level::parse_flags;
use clio::Runtime;
use clio::error::{AmError, ErrorCode, ToolResult};
use clio::retention::RetentionProfile;
use clio::retention_schema::profile_json_schema;
use clio::retention_store::resolve_retention_path;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

/// Dispatch `clio retention <subcommand>`.
pub fn retention(args: impl IntoIterator<Item = String>) -> ExitCode {
    let mut it = args.into_iter();
    let sub = it.next().unwrap_or_default();
    let collected: Vec<String> = it.collect();
    // A help spelling where a subcommand belongs asks for the list of
    // subcommands, the way `clio mcp --help` and `clio ops --help` answer. This
    // runs before the parser, because `--help` is otherwise read as the
    // subcommand name and refused as an unknown retention command.
    if matches!(sub.as_str(), "help" | "--help" | "-h") {
        print_retention_help();
        return ExitCode::SUCCESS;
    }
    // `--help` was never read here: `retention get --help` ran the command and
    // reported the retention store missing, and `retention schema --help` printed
    // the whole JSON schema. Both answered a different question than the one asked.
    // The scan runs before the parser so a help request is not turned into a
    // usage error by a stray token or an unknown flag; `status` and the `mcp`
    // subcommands answer help the same way.
    if retention_subcommands().contains(&sub.as_str())
        && collected
            .iter()
            .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
    {
        println!("{}", retention_help(&sub));
        return ExitCode::SUCCESS;
    }
    let known = retention_known_flags(&sub);
    let flags = match parse_flags(collected.iter().cloned(), &known) {
        Ok(flags) => flags,
        Err(error) => return crate::cli_top_level::report_surface_usage_error(&error, &collected),
    };
    if !retention_subcommands().contains(&sub.as_str()) {
        let subs = retention_subcommands();
        let hint = match crate::cli_args::suggest(&sub, &subs) {
            Some(closest) => format!("did you mean `clio retention {closest}`?"),
            None => "try `clio retention get|set|schema`".to_owned(),
        };
        return eprintln_error(&format!("unknown retention command `{sub}`"), &hint);
    }
    match run(&sub, &flags) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("retention {sub} failed: {e}");
            ExitCode::from(1)
        }
    }
}

/// The `retention` subcommands: the registered `retention` records' first
/// token past the prefix, in registry order. `pub(crate)` so the registry tests
/// assert this derivation's output instead of rebuilding it.
///
/// The split is optional: a one-token suffix is the whole subcommand, and a
/// longer name contributes only its first token, so the derived list can
/// neither skip a record nor carry a mangled name.
pub(crate) fn retention_subcommands() -> Vec<&'static str> {
    crate::cli_command_registry::names()
        .filter_map(|name| name.strip_prefix("retention "))
        .map(|rest| rest.split_once(' ').map_or(rest, |(sub, _)| sub))
        .collect()
}

/// The flags a `clio retention` subcommand accepts: the subcommand's record's
/// own flags. The declared globals are accepted by the shared surface parser's
/// own rule, so a flag neither the record nor a global names is a usage error,
/// and a typo cannot silently fall back to the default bank or store.
///
/// `pub(crate)` so the registry tests assert this derivation's return value
/// instead of rebuilding the list.
pub(crate) fn retention_known_flags(sub: &str) -> Vec<&'static str> {
    match crate::cli_command_registry::find(&format!("retention {sub}")) {
        Some(spec) => spec.documented.iter().map(|flag| flag.long).collect(),
        None => Vec::new(),
    }
}

/// The `clio retention` subcommands, one usage line each.
///
/// This is the screen `clio retention --help` and `clio help retention` print;
/// a subcommand's own flags stay behind `clio retention <subcommand> --help`.
fn print_retention_help() {
    for sub in retention_subcommands() {
        println!(
            "{}",
            crate::cli_help_surfaces::wrapped_usage(&format!("retention {sub}"), 78, 0).join("\n")
        );
    }
    println!("Ask a subcommand for its flags with `clio retention <subcommand> --help`.");
}

/// One retention subcommand's help, read from the shared usage registry.
fn retention_help(sub: &str) -> String {
    let command = format!("retention {sub}");
    let mut text = crate::cli_help_surfaces::wrapped_usage(&command, 78, 0).join("\n");
    if !text.contains('\n') {
        text.push('\n');
    }
    match sub {
        "schema" => {
            text.push_str("\nPrints the retention profile JSON Schema. Takes no flags.\n");
        }
        "get" => {
            text.push_str(
                "\nShows the effective per-bank profile. Without --profiles it reads\n\
                 CLIO_RETENTION_PROFILES.\n",
            );
        }
        _ => {
            text.push_str(
                "\nStores a validated profile. Pass the JSON inline or --profile-file.\n",
            );
        }
    }
    text
}

/// Print a one-line refusal with a hint and exit 2.
fn eprintln_error(message: &str, hint: &str) -> ExitCode {
    eprintln!("{message}\nhint: {hint}");
    ExitCode::from(2)
}

/// Render retention output as pretty JSON.
///
/// # Panics
/// Panics if the payload fails to serialize (the payload types are `Serialize` by
/// construction, so a failure is a bug rather than a runtime condition).
fn pretty<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("retention output serializes")
}

fn ok_data<T: Serialize>(result: ToolResult<T>) -> Result<T, AmError> {
    match result {
        ToolResult::Ok { data, .. } => Ok(data),
        ToolResult::Err { code, message, .. } => Err(AmError::new(code, message)),
    }
}

/// Open a runtime bound to the retention store named by `--profiles` or the env.
fn open_runtime(flags: &HashMap<String, String>) -> Result<Runtime, AmError> {
    let explicit = flags.get("profiles").map(PathBuf::from);
    let path = resolve_retention_path(explicit).ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "no retention store configured; pass `--profiles FILE` or set CLIO_RETENTION_PROFILES",
        )
    })?;
    let mut rt = Runtime::new();
    rt.set_retention_path(path);
    Ok(rt)
}

fn bank_of(flags: &HashMap<String, String>) -> &str {
    flags.get("bank").map_or("default", String::as_str)
}

/// Read the profile JSON from `--profile` (inline) or `--profile-file` (path).
fn profile_from_flags(flags: &HashMap<String, String>) -> Result<RetentionProfile, AmError> {
    let raw = if let Some(inline) = flags.get("profile") {
        inline.clone()
    } else if let Some(path) = flags.get("profile-file") {
        std::fs::read_to_string(path).map_err(|e| {
            AmError::new(
                ErrorCode::InvalidArgument,
                format!("failed to read profile file {path}: {e}"),
            )
        })?
    } else {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "profile is required; pass `--profile '<json>'` or `--profile-file FILE`",
        ));
    };
    serde_json::from_str(&raw).map_err(|e| {
        AmError::new(
            ErrorCode::InvalidType,
            format!("invalid retention profile JSON: {e}"),
        )
    })
}

fn run(sub: &str, flags: &HashMap<String, String>) -> Result<String, AmError> {
    match sub {
        "schema" => Ok(pretty(&profile_json_schema())),
        "get" => {
            let bank = bank_of(flags);
            if bank.is_empty() {
                return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
            }
            require_existing_profiles_file(flags)?;
            let rt = open_runtime(flags)?;
            let data = ok_data(rt.retention_profile_get(bank))?;
            Ok(pretty(&data))
        }
        "set" => {
            let mut rt = open_runtime(flags)?;
            let profile = profile_from_flags(flags)?;
            let data = match flags.get("scope").map_or("bank", String::as_str) {
                "bank" => ok_data(rt.retention_profile_set(bank_of(flags), profile))?,
                "deployment" => ok_data(rt.retention_profile_set_deployment(profile))?,
                other => {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        format!("scope must be bank|deployment, got `{other}`"),
                    ));
                }
            };
            Ok(pretty(&data))
        }
        other => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("unknown retention command `{other}`"),
        )),
    }
}

/// Refuse a configured profiles path that points to no file instead of showing builtin defaults.
fn require_existing_profiles_file(flags: &HashMap<String, String>) -> Result<(), AmError> {
    let explicit = flags.get("profiles").map(PathBuf::from);
    let path = resolve_retention_path(explicit).ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "no retention store configured; pass `--profiles FILE` or set CLIO_RETENTION_PROFILES",
        )
    })?;
    if !path.is_file() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("retention profiles file does not exist: {}", path.display()),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "retention_cli_tests.rs"]
mod retention_cli_tests;
