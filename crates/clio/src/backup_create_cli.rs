// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `clio backup create`: resolving the store to
//! read, taking one consistent snapshot of it, and reporting what the archive
//! holds.
//!
//! ## Owns
//! - The `create` views over its record's flags (`--label`, `--note`,
//!   `--dry-run`, `--keep`, `--confirm`; the accepted set is the
//!   `backup create` registry record's)
//! - The dry-run plan and the wiring from the store half's snapshot and key
//!   member into the archive half's `create_archive`
//! - The keep count's refusals and the one call site that runs the prune after a
//!   successful create, so a create never carries a second copy of the prune
//!   rule
//! - The refusal for a key store the build cannot read, which happens before
//!   any archive is written
//!
//! ## Does not own
//! - Archive assembly and the manifest (see `clio-backup`'s `create`)
//! - The consistent read and the key-store member (see `clio-store`)
//! - Which archives a keep count selects, how a candidate is removed, and the
//!   prune's own report and exit codes (see `backup_prune_cli` and the
//!   `clio-backup` half)
//! - Target and directory resolution (see `backup_cli`)
//!
//! ## Boundary
//!
//! Changes to `clio backup create`'s flags or views should normally require
//! changes here. Changes to how a snapshot is taken should not.
//!
//! Keep this module focused on the `create` command.

use crate::backup_cli::{ResolvedTarget, resolve_backup_dir, resolve_target};
use crate::backup_create_report;
use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_help_usage;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_backup::create::{CreateRequest, KeySource};
use clio_types::AmError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Run `clio backup create` against injected streams, env, and cwd.
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
        let usage = cli_help_usage::usage_for("backup create")
            .unwrap_or_else(|| "usage: clio backup create".to_owned());
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
    // The keep count is refused before the snapshot is taken, so a bad count
    // costs the operator nothing: no archive was written.
    let keep = match crate::backup_prune_cli::keep_count(&parsed) {
        Ok(keep) => keep,
        Err(error) => return fail(io, mode, &error),
    };
    if let Err(error) = confirm_needs_keep(&parsed, keep) {
        return fail(io, mode, &error);
    }
    let get = |key: &str| (io.env)(key);
    let target = match resolve_target(&get, &parsed, "create") {
        Ok(target) => target,
        Err(error) => return fail(io, mode, &error),
    };
    let backup = resolve_backup_dir(&get, &current_dir);
    if parsed.has("dry-run") {
        return dry_run(io, mode, &target, &backup.path, keep);
    }
    match take_snapshot(
        &target,
        &backup.path,
        parsed.value("label"),
        parsed.value("note"),
    ) {
        Ok(created) => emit_created(io, mode, &created, &parsed, keep, &backup.path),
        Err(error) => fail(io, mode, &CliError::from_am_error(&error)),
    }
}

/// Emit the created snapshot, with the `--keep` prune's outcome when one ran.
///
/// The prune is one call site into the prune command's own implementation, not
/// a second copy of the selection and the deletion, so `create --keep` and
/// `clio backup prune` cannot drift apart. A prune that could not remove every
/// candidate becomes the run's exit code, because the snapshot itself did
/// succeed and calling the whole create a failure would be wrong in the other
/// direction.
fn emit_created(
    io: &mut CliIo,
    mode: OutputMode,
    created: &Created,
    parsed: &ParsedArgs,
    keep: Option<usize>,
    backup_dir: &Path,
) -> ExitCode {
    let mut payload = backup_create_report::payload(&created.archive, &created.facts);
    let mut text = backup_create_report::render(&created.archive, &created.facts);
    let mut exit = EXIT_OK;
    if let Some(keep) = keep {
        match crate::backup_prune_cli::run_keep(keep, parsed, backup_dir) {
            Ok(outcome) => {
                payload["prune"] = outcome.payload;
                text.push('\n');
                text.push_str(&outcome.text);
                exit = outcome.exit;
            }
            Err(error) => return fail(io, mode, &error),
        }
    }
    emit(io, mode, &payload, &text);
    ExitCode::from(exit)
}

/// The flags `backup create` accepts: the `backup create` record's own flags,
/// decoded through the registry, plus the globals every parser spec merges.
fn spec() -> FlagSpec {
    match crate::cli_command_registry::find("backup create") {
        Some(spec) => spec.parser_spec(),
        None => FlagSpec::new(&[], &[]),
    }
}

/// Refuse a confirmation with no deletion to confirm.
///
/// `--confirm` means nothing to a create that keeps every snapshot, so it is
/// refused rather than accepted and ignored: a flag that cannot change the run
/// is a trap for whoever reads the command line.
fn confirm_needs_keep(parsed: &ParsedArgs, keep: Option<usize>) -> Result<(), CliError> {
    if keep.is_none() && parsed.has("confirm") {
        return Err(CliError::usage(
            "`clio backup create --confirm` has nothing to confirm: a create deletes nothing",
        )
        .with_hint("add `--keep N` to keep the newest N snapshots after the create"));
    }
    Ok(())
}

/// What one create produced: the archive, and the facts the store half
/// measured. `facts` is backend-shaped for the report but the same view for
/// both backends.
pub(crate) struct Created {
    pub(crate) archive: clio_backup::CreateReport,
    pub(crate) facts: SnapshotView,
}

/// What the report says about the snapshot, whichever backend produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotView {
    /// The backend the snapshot came from.
    pub(crate) backend: String,
    /// The store schema, for Postgres; `None` for SQLite, which has none.
    pub(crate) schema: Option<String>,
    /// The credential-free database or file name.
    pub(crate) database: String,
    /// Copy duration in milliseconds.
    pub(crate) duration_ms: u64,
    /// UTC stamp taken before the copy started.
    pub(crate) started_at: String,
    /// Every counted table with its row count, ordered by table name.
    pub(crate) table_counts: Vec<(String, u64)>,
    /// The store's recorded `schema_version`, when it has one.
    pub(crate) schema_version: Option<String>,
}

impl SnapshotView {
    /// The row count of one table, or `None` when there is no such table.
    pub(crate) fn count_of(&self, table: &str) -> Option<u64> {
        self.table_counts
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, count)| *count)
    }

    /// How many tables were counted.
    pub(crate) fn table_count(&self) -> usize {
        self.table_counts.len()
    }

    /// The view of a SQLite snapshot.
    fn of_sqlite(facts: &clio_store::SnapshotFacts, target: &ResolvedTarget) -> Self {
        Self {
            backend: target.backend.clone(),
            schema: None,
            database: target.database.clone(),
            duration_ms: facts.duration_ms,
            started_at: facts.started_at.clone(),
            table_counts: facts.table_counts.clone(),
            schema_version: facts.schema_version.clone(),
        }
    }
}

/// Take the snapshot, stage it, and assemble the archive.
///
/// This is the one create path, and the backend is its parameter, not a fork:
/// `clio backup restore` takes its pre-restore snapshot through this same
/// function rather than forking a second implementation, which is why `label`
/// and `note` are parameters here and not reads of the parsed flags.
///
/// # Errors
/// The store half's snapshot fails, the key store exists but cannot be read,
/// or the archive cannot be assembled.
pub(crate) fn take_snapshot(
    target: &ResolvedTarget,
    backup_dir: &Path,
    label: Option<&str>,
    note: Option<&str>,
) -> Result<Created, AmError> {
    match target.backend.as_str() {
        "postgres" => {
            crate::backup_create_postgres::take_snapshot_postgres(target, backup_dir, label, note)
        }
        _ => take_snapshot_sqlite(target, backup_dir, label, note),
    }
}

/// The SQLite create path: one staged payload file, one key member.
fn take_snapshot_sqlite(
    target: &ResolvedTarget,
    backup_dir: &Path,
    label: Option<&str>,
    note: Option<&str>,
) -> Result<Created, AmError> {
    clio_backup::staging::ensure_backup_dir(backup_dir)?;
    let staged = clio_backup::staging::reserve_path(backup_dir, "payload")?;
    // The store half writes the payload and measures it in one pass. From here
    // the staged file removes itself on every path out of this function, so a
    // failure never leaves a staged payload behind.
    let facts = clio_store::snapshot_sqlite_into(&target.path, &staged)?;
    let payload = clio_backup::staging::adopt_staged(staged, facts.bytes, facts.sha256.clone());
    // The request resolves the key member, so a key store this build cannot
    // read fails the run here, before `create_archive` is reached: no archive is
    // written, and the staged payload removes itself as the `?` returns. That
    // ordering is the property that makes a refused run leave nothing behind.
    let archive =
        clio_backup::create_archive(&request(target, backup_dir, payload, &facts, label, note)?)?;
    Ok(Created {
        archive,
        facts: SnapshotView::of_sqlite(&facts, target),
    })
}

/// The create request, from the resolved target and the measured snapshot.
///
/// # Errors
/// The key store exists but cannot be read as a key-store database.
fn request(
    target: &ResolvedTarget,
    backup_dir: &Path,
    payload: clio_backup::StagedFile,
    facts: &clio_store::SnapshotFacts,
    label: Option<&str>,
    note: Option<&str>,
) -> Result<CreateRequest, AmError> {
    let slug = clio_config::backup_path::store_slug(target.url.as_str(), target.backend.as_str());
    Ok(CreateRequest {
        backup_dir: backup_dir.to_path_buf(),
        slug,
        backend: target.backend.clone(),
        database: target.database.clone(),
        store_schema: None,
        schema_version: facts.schema_version.clone(),
        payload,
        payload_member: clio_store::PAYLOAD_MEMBER.to_owned(),
        additional_members: Vec::new(),
        method: clio_store::METHOD.to_owned(),
        consistency: clio_store::CONSISTENCY.to_owned(),
        started_at: facts.started_at.clone(),
        duration_ms: facts.duration_ms,
        // Every user table was counted, so the counted set is exhaustive.
        counts_complete: true,
        keys: key_member(target)?,
        label: label.map(str::to_owned),
        note: note.map(str::to_owned),
        tool_version: clio::identity(),
    })
}

/// The key member for this store, or `None` when there is no key-store file.
///
/// A count and a path, never key bytes: the archive half copies the file
/// verbatim and this side only reports where the bytes come from.
///
/// # Errors
/// A key store exists but this build cannot read it. `None` means "there is no
/// key-store file", so turning a read failure into `None` would write an archive
/// that reports itself complete while silently dropping a key store the
/// operator has. The error is returned instead, and the run stops.
pub(crate) fn key_member(target: &ResolvedTarget) -> Result<Option<KeySource>, AmError> {
    let Some(source) = clio_store::key_member_source(target.kms_path.as_deref())? else {
        return Ok(None);
    };
    Ok(Some(KeySource {
        path: source.path,
        subjects: source.subjects,
    }))
}

/// `--dry-run`: print the plan, write nothing.
fn dry_run(
    io: &mut CliIo,
    mode: OutputMode,
    target: &ResolvedTarget,
    backup_dir: &Path,
    keep: Option<usize>,
) -> ExitCode {
    let existing = clio_backup::list_snapshot_rows(backup_dir, None).map_or(0, |rows| rows.len());
    // A SQLite store's size is the file's. A Postgres store's rows are not a
    // file, so its size is unknown until a dump runs: it is reported as `null`,
    // never as a zero that reads as "this costs nothing".
    let (store_bytes, needed) = if target.is_postgres() {
        (Value::Null, Value::Null)
    } else {
        let bytes = std::fs::metadata(&target.path).map_or(0, |meta| meta.len());
        // The staged copy plus the compressed archive, so the plan never
        // understates what the run needs.
        (json!(bytes), json!(bytes.saturating_mul(2)))
    };
    // The Postgres target is measured read-only so the plan can name the schema
    // and the table count, as every Postgres report does. A dry run that cannot
    // reach the target says so instead of failing: it wrote nothing either way.
    let measured = target.is_postgres().then(|| {
        clio_store::postgres_target_state(&target.url)
            .map(|state| (state.schema.clone(), state.table_count()))
            .map_err(|error| error.message)
    });
    let (schema, tables, note) = match &measured {
        Some(Ok((schema, tables))) => (json!(schema), json!(tables), String::new()),
        Some(Err(message)) => (
            Value::Null,
            Value::Null,
            format!("\n  the target was not measured: {message}"),
        ),
        None => (Value::Null, Value::Null, String::new()),
    };
    // Two meanings, two names: the JSON carries the count a script compares, and
    // the text carries the sentence an operator reads. One variable holding both
    // is how a dry run ends up reporting `keep` as prose.
    let keep_note = keep_note(keep);
    let sizes = match (&store_bytes, &needed) {
        (Value::Null, Value::Null) => "store size unknown · space needed unknown".to_owned(),
        (bytes, needed) => format!("store {bytes} B · estimated space needed {needed} B"),
    };
    emit(
        io,
        mode,
        &json!({
            "ok": true,
            "dry_run": true,
            "path": backup_dir.display().to_string(),
            "store": target.display,
            "backend": target.backend,
            "database": target.database,
            "schema": schema,
            "tables": tables,
            "existing_snapshots": existing,
            "keep": keep,
            "store_bytes": store_bytes,
            "estimated_bytes_needed": needed,
        }),
        &format!(
            "dry run: would snapshot {} into {}\n  {sizes} · existing snapshots \
             {existing}{keep_note}{note}",
            target.display,
            backup_dir.display(),
        ),
    );
    ExitCode::from(EXIT_OK)
}

/// The dry run's line about `--keep`.
///
/// A create dry run writes nothing, so it runs no prune either; it says which
/// keep count a real run would apply rather than staying silent about a flag the
/// operator passed.
fn keep_note(keep: Option<usize>) -> String {
    keep.map_or_else(String::new, |keep| format!(" · keep {keep}"))
}

#[cfg(test)]
#[path = "backup_create_cli_tests.rs"]
mod tests;

/// The key-store refusals and the honest keyless case, split out so this file
/// stays readable.
#[cfg(test)]
#[path = "backup_create_cli_key_store_tests.rs"]
mod key_store_tests;

/// The Postgres-backed `create` acceptance tests: the real CLI surface against
/// a scoped-schema Postgres store.
#[cfg(test)]
#[path = "backup_create_cli_pg_tests.rs"]
mod pg_tests;

/// `create --keep N`: the create succeeds, then the prune runs, and the
/// confirmation rule is the prune's own.
#[cfg(test)]
#[path = "backup_create_cli_keep_tests.rs"]
mod keep_tests;
