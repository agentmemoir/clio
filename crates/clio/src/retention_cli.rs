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
//! - Profile resolution semantics or admission scoring
//!
//! ## Boundary
//!
//! Changes to the retention CLI command surface should normally require changes here.
//! Changes to profile semantics should not require changes here.
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
    // `--help` was never read here: `retention get --help` ran the command and
    // reported the retention store missing, and `retention schema --help` printed
    // the whole JSON schema. Both answered a different question than the one asked.
    // The scan runs before the parser so a help request is not turned into a
    // usage error by a stray token or an unknown flag; `status` and the `mcp`
    // subcommands answer help the same way.
    if matches!(sub.as_str(), "get" | "set" | "schema")
        && collected
            .iter()
            .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
    {
        println!("{}", retention_help(&sub));
        return ExitCode::SUCCESS;
    }
    let flags = match parse_flags(collected.iter().cloned(), retention_flags(&sub)) {
        Ok(flags) => flags,
        Err(error) => return crate::cli_top_level::report_surface_usage_error(&error, &collected),
    };
    if !matches!(sub.as_str(), "get" | "set" | "schema") {
        let hint = match crate::cli_args::suggest(&sub, RETENTION_SUBCOMMANDS) {
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

/// The retention subcommands, in the order `clio retention --help` lists them.
const RETENTION_SUBCOMMANDS: &[&str] = &["get", "set", "schema"];

/// The flags each `clio retention` subcommand accepts. Any other flag is a usage
/// error, so a typo cannot silently fall back to the default bank or store.
fn retention_flags(sub: &str) -> &'static [&'static str] {
    match sub {
        "get" => &["profiles", "bank"],
        "set" => &["profiles", "bank", "profile", "profile-file", "scope"],
        _ => &[],
    }
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
            let rt = open_runtime(flags)?;
            let data = ok_data(rt.retention_profile_get(bank_of(flags)))?;
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

#[cfg(test)]
#[path = "retention_cli_tests.rs"]
mod retention_cli_tests;
