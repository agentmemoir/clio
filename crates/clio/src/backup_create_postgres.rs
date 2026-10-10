// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres half of `clio backup create`:
//! driving the store half's one-transaction dump into the backup directory's
//! staged members, and assembling the archive from them.
//!
//! ## Owns
//! - `take_snapshot_postgres`: the staging, the `dump/meta.json`, and the
//!   create request a Postgres snapshot assembles
//! - The cleanup that removes every staged member when the dump fails
//!
//! ## Does not own
//! - The dump transaction itself (see `clio-store`'s postgres dump half)
//! - The SQLite create path and the shared report view (see
//!   `backup_create_cli`, which owns the dispatch and the views)
//! - Archive assembly (see `clio-backup`)
//!
//! ## Boundary
//!
//! Changes to how a Postgres create stages its members should normally
//! require changes here. Changes to the report views or the dispatch should
//! not.
//!
//! Keep this module focused on the Postgres create half.

use crate::backup_cli::ResolvedTarget;
use crate::backup_create_cli::{Created, SnapshotView, key_member};
use clio_backup::create::{CreateRequest, StagedMember};
use clio_backup::dump_meta::{
    DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DumpColumn, DumpMeta, DumpTable,
};
use clio_store::{PG_CONSISTENCY, PG_METHOD};
use clio_types::AmError;
use std::path::{Path, PathBuf};

/// The Postgres create path: one transaction, one staged member per table,
/// `dump/meta.json` as the payload member.
///
/// The dump half hands back one owner-only staged file per table as it
/// discovers them inside its one transaction; this half owns where those
/// files land and removes every one of them when the dump fails, so a
/// cancelled dump leaves nothing in the backup directory. On success the
/// staged members become the archive's additional members in the dump's
/// numbering order, `dump/meta.json` is staged from the dump's facts, and the
/// archive is assembled by the same pipeline the SQLite path uses.
pub(crate) fn take_snapshot_postgres(
    target: &ResolvedTarget,
    backup_dir: &Path,
    label: Option<&str>,
    note: Option<&str>,
) -> Result<Created, AmError> {
    clio_backup::staging::ensure_backup_dir(backup_dir)?;
    let mut staged_paths: Vec<PathBuf> = Vec::new();
    let mut stage = |_table: &clio_store::PgTable,
                     _ordinal: usize|
     -> Result<(PathBuf, std::fs::File), AmError> {
        let (path, file) = clio_backup::staging::begin_staged(backup_dir, "dump-table")?;
        staged_paths.push(path);
        let path = staged_paths
            .last()
            .expect("the path was just pushed")
            .clone();
        Ok((path, file))
    };
    let facts = match clio_store::snapshot_postgres_into(&target.url, &mut stage) {
        Ok(facts) => facts,
        Err(error) => {
            for path in staged_paths {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
    };
    // The dump succeeded, so every staged path is a finished member with the
    // facts the dump recorded; adopting them moves their removal to the
    // archive half's own cleanup.
    let members: Vec<StagedMember> = facts
        .tables
        .iter()
        .zip(staged_paths)
        .map(|(table, path)| StagedMember {
            name: table.member.clone(),
            file: clio_backup::staging::adopt_staged(path, table.bytes, table.sha256.clone()),
        })
        .collect();
    let meta = build_dump_meta(&facts, target);
    meta.validate()?;
    let payload =
        clio_backup::staging::stage_bytes(&meta.to_json_bytes(), backup_dir, "dump-meta")?;
    let request = CreateRequest {
        backup_dir: backup_dir.to_path_buf(),
        slug: clio_config::backup_path::store_slug(target.url.as_str(), target.backend.as_str()),
        backend: target.backend.clone(),
        database: target.database.clone(),
        store_schema: Some(facts.schema.clone()),
        schema_version: facts.schema_version.clone(),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: members,
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        started_at: facts.started_at.clone(),
        duration_ms: facts.duration_ms,
        counts_complete: true,
        keys: key_member(target)?,
        label: label.map(str::to_owned),
        note: note.map(str::to_owned),
        tool_version: clio::identity(),
    };
    let archive = clio_backup::create_archive(&request)?;
    Ok(Created {
        archive,
        facts: view_of_postgres(&facts, target),
    })
}

/// The `dump/meta.json` the dump's facts record.
fn build_dump_meta(facts: &clio_store::PostgresDumpFacts, target: &ResolvedTarget) -> DumpMeta {
    DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: target.backend.clone(),
        schema: facts.schema.clone(),
        database: target.database.clone(),
        extensions: facts.extensions.clone(),
        banks: facts.banks.clone(),
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        tables: facts
            .tables
            .iter()
            .map(|table| DumpTable {
                table: table.table.clone(),
                member: table.member.clone(),
                bytes: table.bytes,
                sha256: table.sha256.clone(),
                rows: table.rows,
                columns: table
                    .columns
                    .iter()
                    .map(|column| DumpColumn {
                        name: column.name.clone(),
                        ordinal: column.ordinal,
                        type_name: column.type_name.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// The report's view of a Postgres snapshot.
fn view_of_postgres(
    facts: &clio_store::PostgresDumpFacts,
    target: &ResolvedTarget,
) -> SnapshotView {
    SnapshotView {
        backend: target.backend.clone(),
        schema: Some(facts.schema.clone()),
        database: target.database.clone(),
        duration_ms: facts.duration_ms,
        started_at: facts.started_at.clone(),
        table_counts: facts
            .tables
            .iter()
            .map(|table| (table.table.clone(), table.rows))
            .collect(),
        schema_version: facts.schema_version.clone(),
    }
}
