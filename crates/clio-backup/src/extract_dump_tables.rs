// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving a Postgres dump's `dump/tables/`
//! members against the dump meta that declares them.
//!
//! ## Owns
//! - Reading the dump meta out of the staged payload member
//! - `declared_table`, the lookup of the one meta record a table member names
//! - Taking a table member: refusing its declared size before a byte is staged,
//!   then proving its streamed size and digest against the meta's record
//! - The refusals for a table member the meta does not declare and for a
//!   declared table the archive does not carry
//!
//! ## Does not own
//! - The archive walk that finds the members (see `extract`)
//! - The payload member's own proof against the manifest (see `extract`)
//! - Staging and journalling a member (see `extract` `stage_recorded`)
//!
//! ## Boundary
//!
//! Changes to how a dump table member is proved should normally require changes
//! here. Changes to the manifest fields should not require changes here.
//!
//! Keep this module focused on proving a dump's table members.

use crate::container::{ArchiveReader, MemberHeader};
use crate::dump_meta::{DumpMeta, DumpTable};
use crate::extract::{ExtractedMember, corrupt, stage_recorded};
use crate::manifest::BackupManifest;
use crate::staging_journal::StagingJournal;
use clio_types::{AmError, ErrorCode};
use std::path::Path;

/// The one table the dump meta declares under `name`.
///
/// A table member the meta does not declare has no declared size and no digest,
/// so it is skipped rather than staged: a restore must never write out rows that
/// nothing in the archive vouches for.
pub(super) fn declared_table<'a>(meta: Option<&'a DumpMeta>, name: &str) -> Option<&'a DumpTable> {
    meta?.tables.iter().find(|table| table.member == name)
}

/// The refusal for a `dump/tables/` member the dump meta does not declare.
pub(super) fn undeclared_table_member(name: &str) -> AmError {
    AmError::new(
        ErrorCode::ArchiveCorrupt,
        format!(
            "the archive carries {name:?}, but the dump meta declares no table member with \
             that name"
        ),
    )
}

/// Read the dump meta out of the staged payload member.
pub(super) fn read_dump_meta(
    payload: &ExtractedMember,
    manifest: &BackupManifest,
) -> Result<DumpMeta, AmError> {
    let bytes = crate::dump_meta::read_bounded_meta_bytes(payload.file.path())?;
    DumpMeta::from_json_bytes(&bytes).map_err(|error| corrupt(&manifest.payload.member, &error))
}

/// Refuse a dump that lost a declared table on the way here.
///
/// Every declared table has to have been staged, with exactly the bytes and
/// digest the meta records, and that is checked member by member as each one
/// streams. A declared table whose member never appeared is a damaged dump, and
/// naming it here is the last chance to do so.
pub(super) fn check_dump_tables_complete(
    meta: &DumpMeta,
    staged: &[String],
) -> Result<(), AmError> {
    for table in &meta.tables {
        if !staged.iter().any(|name| name == &table.member) {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "the dump meta declares table member {:?} for table {:?}, but the archive \
                     does not carry it",
                    table.member, table.table
                ),
            ));
        }
    }
    Ok(())
}

/// Take the open table member: the dump meta's declared size is checked before
/// anything is inflated, then the streamed size and digest are proved against
/// the meta's own record.
///
/// The size check comes first for the same reason the payload's does: a member
/// whose header declares more bytes than the dump meta records is not the
/// member the meta describes, so no reason exists to write it to disk. The
/// staged file is dropped on the refusal path, so the check is about not
/// writing untrusted bytes at all, not about cleaning up afterwards.
pub(super) fn take_dump_table(
    reader: &mut ArchiveReader<std::fs::File>,
    header: &MemberHeader,
    table: &DumpTable,
    dir: &Path,
    journal: &mut Option<StagingJournal>,
) -> Result<ExtractedMember, AmError> {
    if header.size != table.bytes {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "archive member {:?}: the archive header declares {} bytes, but the dump meta \
                 records {} bytes for table {:?}, so the member cannot be the one the dump meta \
                 describes",
                header.name, header.size, table.bytes, table.table
            ),
        ));
    }
    let staged = stage_recorded(reader, dir, "restore-table", &header.name, journal)?;
    let mut problems: Vec<&str> = Vec::new();
    if staged.bytes() != table.bytes {
        problems.push("size");
    }
    if staged.sha256() != table.sha256 {
        problems.push("digest");
    }
    if !problems.is_empty() {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "archive member {:?}: its {} for table {:?} {} not match the dump meta's record \
                 (meta: {} bytes, digest {}; member: {} bytes, digest {})",
                header.name,
                problems.join(" and the "),
                if problems.len() == 1 { "does" } else { "do" },
                table.table,
                table.bytes,
                table.sha256,
                staged.bytes(),
                staged.sha256()
            ),
        ));
    }
    Ok(ExtractedMember {
        member: header.name.clone(),
        file: staged,
    })
}
