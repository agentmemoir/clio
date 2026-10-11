// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `clio backup list`: showing every snapshot
//! in the backup directory from its manifest alone, newest first.
//!
//! ## Owns
//! - The `list` flags (`--limit`) and its text and JSON views
//! - The refusal for the store flags `list` cannot honor
//!
//! ## Does not own
//! - Reading an archive's manifest (see `clio-backup`'s `inventory`)
//! - Directory resolution (see `backup_cli`)
//! - Verifying an archive (see `clio-backup`'s `verify`)
//!
//! ## Boundary
//!
//! Changes to `clio backup list`'s flags or views should normally require
//! changes here. Changes to how archives are discovered should not.
//!
//! Keep this module focused on the `list` command.

use crate::backup_cli::resolve_backup_dir;
use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_help_usage;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_backup::SnapshotRow;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::ExitCode;

/// The value flags `list` adds to the globals.
const VALUE_FLAGS: &[&str] = &["limit"];

/// Run `clio backup list` against injected streams, env, and cwd.
pub(crate) fn run(
    io: &mut CliIo,
    fallback: OutputMode,
    tokens: impl IntoIterator<Item = String>,
    current_dir: Result<PathBuf, CliError>,
) -> ExitCode {
    let current_dir = match current_dir {
        Ok(dir) => dir,
        Err(error) => return fail(io, fallback, &error),
    };
    let parsed = match cli_args::parse(&spec(), tokens) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    if parsed.has("help") {
        let usage = cli_help_usage::usage_for("backup list")
            .unwrap_or_else(|| "usage: clio backup list".to_owned());
        let _ = cli_output::write_text(&mut io.out, &usage);
        return ExitCode::from(EXIT_OK);
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Err(error) = crate::runtime_cli::no_positionals(&parsed) {
        return fail(io, mode, &error);
    }
    if let Err(error) = refuse_store_flags(&parsed) {
        return fail(io, mode, &error);
    }
    let limit = match limit(&parsed) {
        Ok(limit) => limit,
        Err(error) => return fail(io, mode, &error),
    };
    let get = |key: &str| (io.env)(key);
    let backup = resolve_backup_dir(&get, &current_dir);
    // A listing reads archives, never a store, so a missing directory is an
    // empty listing rather than a failure: the operator gets a truthful zero.
    let rows = match clio_backup::list_snapshot_rows(&backup.path, limit) {
        Ok(rows) => rows,
        Err(error) if error.code == clio_types::ErrorCode::NotFound => Vec::new(),
        Err(error) => return fail(io, mode, &CliError::from_am_error(&error)),
    };
    emit(io, mode, &payload(&rows), &render(&rows));
    ExitCode::from(EXIT_OK)
}

/// The flags `backup list` accepts.
fn spec() -> FlagSpec {
    FlagSpec::new(VALUE_FLAGS, &[])
}

/// `--limit`, refused when it is not a count.
fn limit(parsed: &ParsedArgs) -> Result<Option<usize>, CliError> {
    match parsed.value("limit") {
        None => Ok(None),
        Some(raw) => raw.parse::<usize>().map(Some).map_err(|_| {
            CliError::usage(format!("`--limit` takes a count, got `{raw}`"))
                .with_hint("try `--limit 10`")
        }),
    }
}

/// Refuse the store flags a listing cannot honor.
///
/// `list` reads the manifest member of each archive and opens no database, so
/// `--db` and `--backend` would be silently ignored. Saying so beats accepting a
/// flag that cannot change the answer.
fn refuse_store_flags(parsed: &ParsedArgs) -> Result<(), CliError> {
    for flag in ["db", "backend"] {
        if parsed.value(flag).is_some() {
            return Err(CliError::usage(format!(
                "`clio backup list` reads snapshots, not a store; `--{flag}` cannot change the \
                 listing"
            ))
            .with_hint("drop the flag, or use `clio backup create` to take a snapshot"));
        }
    }
    Ok(())
}

/// The JSON listing: every row, in listing order.
fn payload(rows: &[SnapshotRow]) -> Value {
    json!({
        "ok": true,
        "count": rows.len(),
        "snapshots": rows,
    })
}

/// The human listing: one line per snapshot, newest first.
fn render(rows: &[SnapshotRow]) -> String {
    if rows.is_empty() {
        return "no snapshots in the backup directory".to_owned();
    }
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(reason) = &row.unreadable {
            lines.push(format!("{} unreadable: {reason}", row.id));
            continue;
        }
        let label = row.label.as_deref().unwrap_or("");
        let keys = if row.keys_included { "keys" } else { "no keys" };
        let label = if label.is_empty() {
            String::new()
        } else {
            format!(" · {label}")
        };
        lines.push(format!(
            "{} {} · {} · payload {} B · archive {} B · {keys}{label}",
            row.id, row.created_at, row.backend, row.payload_bytes, row.archive_bytes
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "backup_list_cli_tests.rs"]
mod tests;

/// Mixed-backend listings and verify, against a hand-built Postgres archive.
#[cfg(test)]
#[path = "backup_list_cli_mixed_tests.rs"]
mod mixed_tests;
