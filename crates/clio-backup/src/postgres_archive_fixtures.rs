// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hand-built Postgres archive fixtures the
//! archive suites share: a dump meta over named tables, the assembled archive,
//! its member names, and the extraction the product itself runs.
//!
//! ## Owns
//! - `dump_meta`, `postgres_archive`, `member_names`, and `extract`
//!
//! ## Does not own
//! - The assertions built on them (see `postgres_archive_tests`)
//! - The SQLite archive fixtures (see `fixtures`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::create::{CreateRequest, StagedMember};
use crate::dump_meta::{
    DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DUMP_TABLES_PREFIX, DumpColumn, DumpMeta,
    DumpTable,
};
use crate::staging;
use clio_types::AmError;
use std::path::Path;

pub(crate) use crate::fixtures::Scratch;

/// A dump meta over `tables`: one row per table, in name order, with the
/// staged member's own measured digest recorded.
pub(crate) fn dump_meta(
    scratch: &Scratch,
    tables: &[(&str, &[u8])],
) -> (DumpMeta, Vec<StagedMember>) {
    let mut entries = Vec::new();
    let mut members = Vec::new();
    for (index, (table, rows)) in tables.iter().enumerate() {
        let member = format!("{DUMP_TABLES_PREFIX}{:03}_{table}.txt", index + 1);
        let staged = staging::stage_bytes(rows, scratch.path(), "dump-table").expect("stage rows");
        entries.push(DumpTable {
            table: (*table).to_owned(),
            member: member.clone(),
            bytes: staged.bytes(),
            sha256: staged.sha256().to_owned(),
            rows: 1,
            columns: vec![DumpColumn {
                name: "id".to_owned(),
                ordinal: 1,
                type_name: "text".to_owned(),
            }],
        });
        members.push(StagedMember {
            name: member,
            file: staged,
        });
    }
    let meta = DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: vec!["vector".to_owned()],
        banks: vec!["default".to_owned()],
        method: "postgres-copy".to_owned(),
        consistency: "point-in-time".to_owned(),
        tables: entries,
    };
    (meta, members)
}

/// Assemble a Postgres archive from a meta and its table members.
pub(crate) fn postgres_archive(
    scratch: &Scratch,
    meta: &DumpMeta,
    members: Vec<StagedMember>,
) -> crate::create::CreateReport {
    let payload = staging::stage_bytes(&meta.to_json_bytes(), scratch.path(), "dump-meta")
        .expect("stage the dump meta");
    let request = CreateRequest {
        backup_dir: scratch.path().to_path_buf(),
        slug: "clio".to_owned(),
        backend: "postgres".to_owned(),
        database: meta.database.clone(),
        store_schema: Some(meta.schema.clone()),
        schema_version: Some("1".to_owned()),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: members,
        method: meta.method.clone(),
        consistency: meta.consistency.clone(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 42,
        counts_complete: true,
        keys: None,
        label: None,
        note: None,
        tool_version: "test".to_owned(),
    };
    crate::create_archive(&request).expect("assemble the archive")
}

/// Read every member name out of one archive, in order.
pub(crate) fn member_names(archive: &Path) -> Vec<String> {
    let file = std::fs::File::open(archive).expect("open the archive");
    let mut reader = crate::container::ArchiveReader::new(file).expect("reader");
    let mut names = Vec::new();
    while let Some(header) = reader.next_member().expect("member") {
        names.push(header.name);
        reader.skip_current().expect("skip the member");
    }
    names
}

/// Extract an archive with the manifest it carries, as the product does: one
/// read of the manifest feeds every decision and every staged byte.
pub(crate) fn extract(
    archive: &Path,
    out: &Path,
) -> Result<crate::extract::ExtractedArchive, AmError> {
    let manifest = crate::inventory::read_manifest(archive).expect("read the manifest");
    crate::extract::extract_members(archive, &manifest, out)
}
