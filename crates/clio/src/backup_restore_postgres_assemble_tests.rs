// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the shared harness for building a hand-assembled Postgres
//! archive from a real dump: the three assemblers the restore tests drive, and
//! the two fixture shapes (a rewritten items member, a declared-but-absent
//! table) their refusal scenarios need.
//!
//! ## Owns
//! - `assemble_pg_archive`, `assemble_pg_archive_with_keys`, and the
//!   `build_pg_archive` both call
//! - `rewrite_items_member` and `phantom_table_dump`, the two ways a test makes
//!   an archive lie about itself
//!
//! ## Does not own
//! - The archive writer (see `clio-backup`'s create half)
//! - The restore scenarios that call these helpers (see this crate's
//!   `backup_restore_postgres_refusal_tests` and `backup_restore_postgres_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_backup::create::KeySource;
use clio_store::PostgresDumpFacts;
use std::path::{Path, PathBuf};

use crate::backup_cli_fixtures::ensure_journal_in;

/// Assemble one Postgres archive from a real dump's facts, through the
/// archive half's public API.
///
/// `rewrite_items` replaces the items member's bytes with raw content the
/// items table refuses, and `phantom_table` declares a table the archive does
/// not carry; both are the shapes the restore's own refusals exist for.
pub(crate) fn assemble_pg_archive(
    backup_dir: &Path,
    facts: &PostgresDumpFacts,
    staged_paths: &[PathBuf],
    rewrite_items: Option<&[u8]>,
    phantom_table: bool,
) -> String {
    build_pg_archive(
        backup_dir,
        facts,
        staged_paths,
        rewrite_items,
        phantom_table,
        None,
    )
}

/// As `assemble_pg_archive`, and the archive also carries a key member copied
/// from `kms_path`, so a restore attempts the key swap instead of refusing for
/// a missing key store.
pub(crate) fn assemble_pg_archive_with_keys(
    backup_dir: &Path,
    facts: &PostgresDumpFacts,
    staged_paths: &[PathBuf],
    rewrite_items: Option<&[u8]>,
    phantom_table: bool,
    kms_path: Option<&Path>,
) -> String {
    build_pg_archive(
        backup_dir,
        facts,
        staged_paths,
        rewrite_items,
        phantom_table,
        kms_path,
    )
}

/// The one assembler both wrappers call. `kms_path` is the key store copied in
/// as the archive's key member, or `None` for an archive that carries none.
fn build_pg_archive(
    backup_dir: &Path,
    facts: &PostgresDumpFacts,
    staged_paths: &[PathBuf],
    rewrite_items: Option<&[u8]>,
    phantom_table: bool,
    kms_path: Option<&Path>,
) -> String {
    use clio_backup::dump_meta::{
        DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DumpColumn, DumpMeta, DumpTable,
    };
    let mut facts = facts.clone();
    if let Some(bytes) = rewrite_items {
        rewrite_items_member(&mut facts, staged_paths, backup_dir, bytes);
    }
    if phantom_table {
        facts.tables.push(phantom_table_dump());
    }
    let members: Vec<clio_backup::create::StagedMember> = facts
        .tables
        .iter()
        .zip(staged_paths)
        .map(|(table, path)| clio_backup::create::StagedMember {
            name: table.member.clone(),
            file: clio_backup::staging::adopt_staged(
                path.clone(),
                table.bytes,
                table.sha256.clone(),
            ),
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
        method: clio_store::PG_METHOD.to_owned(),
        consistency: clio_store::PG_CONSISTENCY.to_owned(),
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
    let payload = clio_backup::staging::stage_bytes(&meta.to_json_bytes(), backup_dir, "dump-meta")
        .expect("stage the dump meta");
    let request = clio_backup::create::CreateRequest {
        backup_dir: backup_dir.to_path_buf(),
        slug: "clio".to_owned(),
        backend: "postgres".to_owned(),
        database: "clio".to_owned(),
        store_schema: Some(meta.schema),
        schema_version: facts.schema_version.clone(),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: members,
        // A `None` path makes the archive carry no key member, so a restore
        // refuses for a missing key store instead of attempting a swap.
        keys: kms_path.map(|path| KeySource {
            path: path.to_path_buf(),
            subjects: 2,
        }),
        method: clio_store::PG_METHOD.to_owned(),
        consistency: clio_store::PG_CONSISTENCY.to_owned(),
        started_at: facts.started_at.clone(),
        duration_ms: facts.duration_ms,
        counts_complete: true,
        label: None,
        note: None,
        authority: Some(ensure_journal_in(
            backup_dir
                .parent()
                .expect("the backup directory has a parent"),
        )),
        tool_version: clio::identity(),
    };
    let report = clio_backup::create_archive(&request).expect("assemble the archive");
    report.id
}

/// Replace the items member's staged bytes and restate its size and digest, so
/// the archive declares a member whose content the dump never produced.
fn rewrite_items_member(
    facts: &mut clio_store::PostgresDumpFacts,
    staged_paths: &[PathBuf],
    backup_dir: &Path,
    bytes: &[u8],
) {
    let index = facts
        .tables
        .iter()
        .position(|table| table.table == "items")
        .expect("an items table");
    std::fs::write(&staged_paths[index], bytes).expect("rewrite the staged items member");
    // The staged file's digest comes from the staging half, which owns the
    // digest rules; the staged scratch file removes itself on drop.
    let staged = clio_backup::staging::stage_bytes(bytes, backup_dir, "rewrite")
        .expect("stage the rewritten member");
    facts.tables[index].bytes = staged.bytes();
    facts.tables[index].sha256 = staged.sha256().to_owned();
    drop(staged);
}

/// A table the dump meta declares and the archive carries no member for, which
/// is the shape the restore's table-set pre-flight exists to refuse.
fn phantom_table_dump() -> clio_store::TableDump {
    clio_store::TableDump {
        table: "phantom_table".to_owned(),
        member: "dump/tables/099_phantom_table.txt".to_owned(),
        bytes: 0,
        sha256: "0".repeat(64),
        rows: 1,
        columns: vec![clio_store::PgColumn {
            name: "id".to_owned(),
            ordinal: 1,
            type_name: "text".to_owned(),
        }],
    }
}
