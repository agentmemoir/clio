// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `clio backup verify`: resolving which archive
//! an id names, and reporting the per-member verdict.
//!
//! ## Owns
//! - The `verify` flags (`--latest`) and its text and JSON views
//! - Both views stating the archive's backend, snapshot method, consistency,
//!   and store schema alongside the per-member verdict
//! - Id resolution, including the closest-id hint for an unknown id
//! - The refusals for flags and backends this build cannot honor
//!
//! ## Does not own
//! - Reading members and comparing digests (see `clio-backup`'s `verify`)
//! - The store's integrity check (see `clio-store`; it is called, not
//!   reimplemented)
//! - Directory resolution (see `backup_cli`)
//!
//! ## Boundary
//!
//! Changes to `clio backup verify`'s flags or views should normally require
//! changes here. Changes to what a verify pass checks should not.
//!
//! Keep this module focused on the `verify` command.

use crate::backup_cli::resolve_backup_dir;
use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK, EXIT_OPERATIONAL};
use crate::cli_help_usage;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_backup::SnapshotRow;
use clio_backup::verify::VerifyReport;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The value flags `verify` adds to the globals (none beyond the globals).
const VALUE_FLAGS: &[&str] = &[];

/// The boolean flags `verify` adds to the globals: the newest-snapshot
/// selector.
const BOOL_FLAGS: &[&str] = &["latest"];

/// Run `clio backup verify` against injected streams, env, and cwd.
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
        let usage =
            cli_help_usage::usage_for("backup verify").unwrap_or("usage: clio backup verify");
        let _ = cli_output::write_text(&mut io.out, usage);
        return ExitCode::from(EXIT_OK);
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Err(error) = one_selector(&parsed) {
        return fail(io, mode, &error);
    }
    if let Err(error) = refuse_store_flags(&parsed) {
        return fail(io, mode, &error);
    }
    let get = |key: &str| (io.env)(key);
    let backup = resolve_backup_dir(&get, &current_dir);
    let rows = match clio_backup::list_snapshot_rows(&backup.path, None) {
        Ok(rows) => rows,
        Err(error) if error.code == clio_types::ErrorCode::NotFound => Vec::new(),
        Err(error) => return fail(io, mode, &CliError::from_am_error(&error)),
    };
    let id = match resolve_id(&parsed, &rows) {
        Ok(id) => id,
        Err(error) => return fail(io, mode, &error),
    };
    let archive = backup.path.join(clio_backup::archive_file_name(&id));
    match verify(&archive) {
        Ok(report) => {
            emit(io, mode, &payload(&report), &render(&report));
            if report.ok {
                ExitCode::from(EXIT_OK)
            } else {
                ExitCode::from(EXIT_OPERATIONAL)
            }
        }
        Err(error) => fail(io, mode, &error),
    }
}

/// The flags `backup verify` accepts.
fn spec() -> FlagSpec {
    FlagSpec::new(VALUE_FLAGS, BOOL_FLAGS)
}

/// Exactly one selector: a snapshot id, or `--latest`.
fn one_selector(parsed: &ParsedArgs) -> Result<(), CliError> {
    let positionals = parsed.positionals().len();
    let latest = parsed.has("latest");
    if positionals > 1 || (positionals == 1 && latest) {
        return Err(CliError::usage(
            "`clio backup verify` takes one snapshot id, or --latest, not both",
        )
        .with_hint("try `clio backup verify --latest`"));
    }
    if positionals == 0 && !latest {
        return Err(
            CliError::usage("`clio backup verify` needs a snapshot id, or --latest")
                .with_hint("run `clio backup list` to see the ids"),
        );
    }
    Ok(())
}

/// Refuse the store flags a verify pass cannot honor.
///
/// `verify` reads an archive, not a store, so `--db` cannot change the answer,
/// and `--backend` names a store the archive is not.
fn refuse_store_flags(parsed: &ParsedArgs) -> Result<(), CliError> {
    if parsed.value("db").is_some() {
        return Err(CliError::usage(
            "`clio backup verify` reads an archive, not a store; --db cannot change the answer",
        )
        .with_hint("drop the flag, or run `clio backup list` to see the snapshots"));
    }
    if let Some(backend) = parsed.value("backend") {
        return Err(CliError::usage(format!(
            "`clio backup verify` reads an archive whose manifest states its backend; \
             --backend {backend} cannot change what it reads"
        ))
        .with_hint("drop the flag, or run `clio backup list` to see the snapshots"));
    }
    Ok(())
}

/// Verify one archive, with each backend's own payload check behind it.
///
/// A SQLite payload is checked by the store's own integrity check; a Postgres
/// archive's payload is its `dump/meta.json`, so the check is that member's
/// own validation. The manifest is read first, bounded to the first member: it
/// decides whether this build can verify the archive at all, and a payload is
/// only inflated for an archive that can be checked.
fn verify(path: &Path) -> Result<VerifyReport, CliError> {
    // The manifest is read and validated first, so the backend is already one
    // this build knows: a Postgres archive's payload is its dump meta, a
    // SQLite payload is checked by the store's own integrity check.
    let manifest = clio_backup::read_manifest(path).map_err(|e| CliError::from_am_error(&e))?;
    if manifest.store.backend == "postgres" {
        clio_backup::verify_archive(path, &mut |payload| {
            clio_backup::dump_meta::validate_dump_meta_file(payload)
        })
        .map_err(|e| CliError::from_am_error(&e))
    } else {
        clio_backup::verify_archive(path, &mut |payload| {
            clio_store::sqlite_integrity_ok(payload)
        })
        .map_err(|e| CliError::from_am_error(&e))
    }
}

/// The id to verify: the positional, or the newest row of the listing.
fn resolve_id(parsed: &ParsedArgs, rows: &[SnapshotRow]) -> Result<String, CliError> {
    if parsed.has("latest") {
        return rows.first().map(|row| row.id.clone()).ok_or_else(|| {
            CliError::operational(
                "not_found",
                "no snapshot to verify: the backup directory is empty",
            )
            .with_hint("take one with `clio backup create`")
        });
    }
    let id = parsed.positional(0).unwrap_or_default();
    if rows.iter().any(|row| row.id == id) {
        return Ok(id.to_owned());
    }
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let hint = cli_args::suggest(id, &ids).map_or_else(
        || "run `clio backup list` to see the snapshots".to_owned(),
        |best| format!("did you mean `{best}`?"),
    );
    Err(CliError::operational(
        "not_found",
        format!("no snapshot `{id}` in the backup directory"),
    )
    .with_hint(hint))
}

/// The JSON verdict: per member, plus the archive's own facts.
///
/// The backend, the method, the consistency, and the schema come from the
/// archive's own manifest, so a report always states which store it read and
/// how that store was copied — a pass with no method is a pass nobody can
/// weigh.
fn payload(report: &VerifyReport) -> Value {
    json!({
        "ok": report.ok,
        "snapshot_id": report.id,
        "path": report.path.display().to_string(),
        "created_at": report.created_at,
        "backend": report.backend,
        "method": report.method,
        "consistency": report.consistency,
        "schema": report.store_schema,
        "counts_complete": report.counts_complete,
        "unlisted_members": report.unlisted,
        "failed": report.failed(),
        "members": report.members,
    })
}

/// The human verdict: the id and the path, a line naming the backend, the
/// method, and the schema (when the backend has one), then one line per member
/// check.
///
/// The archive's own facts come first, so a text-mode pass says what the JSON
/// does rather than leaving the operator to open the archive to learn which
/// store was read and how it was copied.
fn render(report: &VerifyReport) -> String {
    let schema = report
        .store_schema
        .as_deref()
        .map_or_else(String::new, |schema| format!(" · schema {schema}"));
    let mut lines = vec![
        format!(
            "verify {} ({}) {}",
            report.id,
            report.path.display(),
            if report.ok { "ok" } else { "FAILED" }
        ),
        format!(
            "  {} snapshot · method {} · {}{schema}",
            report.backend, report.method, report.consistency
        ),
    ];
    for check in &report.members {
        lines.push(format!(
            "  {} {} · {}",
            if check.ok { "pass" } else { "FAIL" },
            check.member,
            check.detail
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "backup_verify_cli_tests.rs"]
mod tests;
