// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres-specific shared fixtures of the
//! old-archive restore measurement: the target URL, its reachability probe, the
//! dump archive assembly, and the row-load restore.
//!
//! ## Owns
//! - The Postgres URL resolution and required-backend gate
//! - `archive_postgres`: a dump plus its key member assembled into an archive
//! - `restore_pg`: a dump loaded into a target schema, with or without its key
//!
//! ## Does not own
//! - The shared fixtures and probes (see `support`)
//! - The Postgres scenarios and their assertions (see `postgres`, `members`)
//!
//! ## Boundary
//!
//! Changes to how a Postgres archive is assembled or restored should normally
//! require changes here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on the Postgres fixtures.

use crate::support::{Archive, extract_into, key_source, publish};
use clio::AmError;
use clio::store::{
    PgTable, PostgresRestoreReport, PostgresRestoreRequest, RestoreTable, ScopedSchema,
    place_staged_key, postgres_target_state, restore_postgres,
};
use clio_backup::dump_meta::{
    DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DumpColumn, DumpMeta, DumpTable,
};
use clio_backup::manifest::ManifestAuthority;
use clio_backup::staging;
use clio_backup::{CreateRequest, StagedMember, create_archive};
use std::path::{Path, PathBuf};

/// The Postgres URL used when the environment names none, matching the Makefile.
const COMPOSE_PG: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// The name the skip note carries for this measurement.
const SUITE: &str = "the postgres old-archive measurement";

/// The Postgres URL to use: the environment's, when it names a Postgres
/// database, or the local Compose stack.
///
/// The store's gate owns the refusal of a value that names another backend:
/// dialling it would report an unreachable Postgres for a URL that never named
/// one. The gate reads the operator's opt-out before it refuses, so a machine
/// that set `CLIO_SKIP_PG_TESTS` skips instead of failing. The refused value
/// is kept unchanged in that case, so its probe fails and the arm skips.
pub fn pg_url() -> String {
    match clio_config::db_url_env::database_url_from_process_env() {
        Ok(url) => match clio::store::pg_skip::pg_url_or_skip(SUITE) {
            Some(accepted) => accepted,
            None => url,
        },
        Err(_) => COMPOSE_PG.to_owned(),
    }
}

/// True when a Postgres server answers at `url` within a short budget.
///
/// Delegates to the store's test-support probe, so this harness, the other
/// command harnesses, and the redacted endpoint all read the URL the same way.
pub fn postgres_reachable(url: &str) -> bool {
    clio::store::pg_test_target::postgres_reachable(url)
}

/// Require a reachable Postgres server for a Postgres measurement.
///
/// `reachable` is the caller's probe of the configured URL. When it is `true`
/// this returns `true` and the caller proceeds. When it is `false`, Postgres is
/// missing: this fails loudly, because Postgres is a required backend of this
/// measurement, and a test that returned green without it would report a no-op
/// as a result. Only the explicit `CLIO_SKIP_PG_TESTS` opt-out turns the
/// absence into a printed skip and a `false` return. The URL is never printed,
/// because it can carry a password into the test log.
pub fn require_postgres(reachable: bool) -> bool {
    if reachable {
        return true;
    }
    clio::store::pg_skip::pg_required_or_skip(
        SUITE,
        "no Postgres answers at CLIO_DATABASE_URL (start the database, or set \
         CLIO_SKIP_PG_TESTS=1 to run the rest of the suite without it)",
    );
    false
}

/// Assemble a Postgres archive from the live store at `url`, carrying `kms_path`.
pub fn archive_postgres(backup_dir: &Path, url: &str, kms_path: &Path, slug: &str) -> Archive {
    archive_postgres_with_authority(backup_dir, url, kms_path, slug, None)
}

/// Assemble a Postgres archive that records the erasure `authority` it was made
/// under, so the guarded restore can classify its claim.
pub fn archive_postgres_with_authority(
    backup_dir: &Path,
    url: &str,
    kms_path: &Path,
    slug: &str,
    authority: Option<ManifestAuthority>,
) -> Archive {
    staging::ensure_backup_dir(backup_dir).expect("create the backup directory");
    let mut staged_paths: Vec<PathBuf> = Vec::new();
    let mut stage =
        |_table: &PgTable, _ordinal: usize| -> Result<(PathBuf, std::fs::File), AmError> {
            let (path, file) = staging::begin_staged(backup_dir, "dump-table")?;
            staged_paths.push(path.clone());
            Ok((path, file))
        };
    let facts =
        clio::store::snapshot_postgres_into(url, &mut stage).expect("snapshot the postgres store");
    let members: Vec<StagedMember> = facts
        .tables
        .iter()
        .zip(staged_paths)
        .map(|(table, path)| StagedMember {
            name: table.member.clone(),
            file: staging::adopt_staged(path, table.bytes, table.sha256.clone()),
        })
        .collect();
    let meta = DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: facts.schema.clone(),
        database: "clio".to_owned(),
        extensions: facts.extensions.clone(),
        banks: facts.banks.clone(),
        method: clio::store::PG_METHOD.to_owned(),
        consistency: clio::store::PG_CONSISTENCY.to_owned(),
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
    };
    meta.validate().expect("validate the dump meta");
    let payload = staging::stage_bytes(&meta.to_json_bytes(), backup_dir, "dump-meta")
        .expect("stage the dump meta");
    let request = CreateRequest {
        backup_dir: backup_dir.to_path_buf(),
        slug: slug.to_owned(),
        backend: "postgres".to_owned(),
        database: "clio".to_owned(),
        store_schema: Some(facts.schema.clone()),
        schema_version: facts.schema_version.clone(),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: members,
        method: clio::store::PG_METHOD.to_owned(),
        consistency: clio::store::PG_CONSISTENCY.to_owned(),
        started_at: facts.started_at.clone(),
        duration_ms: facts.duration_ms,
        counts_complete: true,
        keys: key_source(kms_path),
        label: None,
        note: None,
        authority,
        tool_version: clio::version().to_owned(),
    };
    publish(create_archive(&request).expect("create the postgres archive"))
}

/// Load a Postgres archive into `target`'s schema, then place the key member
/// when `with_key`. Returns the load report.
pub fn restore_pg(
    archive: &Archive,
    target: &ScopedSchema,
    target_kms: &Path,
    extract_dir: &Path,
    with_key: bool,
) -> PostgresRestoreReport {
    std::fs::create_dir_all(extract_dir).expect("create the extract directory");
    let extracted = extract_into(archive, extract_dir);
    let meta = DumpMeta::read_from_archive(&archive.path).expect("read the dump meta");
    let state = postgres_target_state(target.url()).expect("measure the target schema");
    let tables = meta
        .tables
        .iter()
        .zip(extracted.dump_tables.iter())
        .map(|(table, member)| RestoreTable {
            table: table.table.clone(),
            columns: table.columns.iter().map(|c| c.name.clone()).collect(),
            member_path: member.file.path().to_path_buf(),
            rows: table.rows,
        })
        .collect();
    let report = restore_postgres(&PostgresRestoreRequest {
        url: target.url().to_owned(),
        schema: state.schema,
        tables,
    })
    .expect("load the postgres archive");
    if with_key && let Some(member) = &extracted.key {
        place_staged_key(member.file.path(), target_kms);
    }
    report
}
