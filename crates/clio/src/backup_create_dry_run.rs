// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `clio backup create --dry-run`: the plan it
//! prints and the guarantee that it writes nothing.
//!
//! ## Owns
//! - The dry-run plan: the target, the backup directory, the sizes, the keep
//!   count, and the measured Postgres schema and table count
//!
//! ## Does not own
//! - Taking the snapshot or writing the archive (`backup_create_cli` owns that)
//! - The keep count's refusals or the prune (the prune command owns those)
//!
//! ## Boundary
//!
//! Changes to what a dry run prints should normally require changes here.
//! Changes to how a snapshot is taken should not.
//!
//! Keep this module focused on the create dry-run plan.

use crate::backup_cli::ResolvedTarget;
use crate::cli_error::EXIT_OK;
use crate::cli_output::OutputMode;
use crate::cli_read::{CliIo, emit};
use serde_json::{Value, json};
use std::path::Path;
use std::process::ExitCode;

/// `--dry-run`: print the plan, write nothing.
pub(crate) fn dry_run(
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
