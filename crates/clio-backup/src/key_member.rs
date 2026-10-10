// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for taking an archive's key member out and
//! proving what it is: the manifest must declare it, and its head must be a
//! SQLite key-store database rather than any other member the archive carries.
//!
//! ## Owns
//! - `take_key_member` and the checks it runs: the manifest's declaration, the
//!   recorded size against the header's, and the SQLite-magic head
//!
//! ## Does not own
//! - The member walk that calls it (see `extract`)
//! - The payload and dump-table members (see `extract` and `dump_meta`)
//!
//! ## Boundary
//!
//! Changes to what a key member must look like should normally require changes
//! here. Changes to the rest of the member walk should not.
//!
//! Keep this module focused on the one member that carries key material.

use crate::KEY_MEMBER;
use crate::container::{ArchiveReader, MemberHeader};
use crate::extract::{ExtractedMember, stage_recorded, staging_failure};
use crate::manifest::BackupManifest;
use crate::staging::{self, StagedFile};
use crate::staging_journal::StagingJournal;
use clio_types::{AmError, ErrorCode};
use std::io::Read;
use std::path::Path;

/// Bytes read back from a staged key member's head to recognize SQLite.
const MAGIC_LEN: usize = 16;

/// SQLite file magic; a key-store member must start with it.
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// Take the open key member: the manifest must declare it, the header's size
/// must match the size the manifest records, and its head must be a SQLite
/// database. The manifest records no digest for this member, so the digest is
/// only reported by the staged file.
///
/// The size check comes before this member is staged, like the payload's does:
/// a member whose header declares a different length than the manifest records
/// is not the key member the manifest describes, so there is no reason to
/// write it to disk, and a refusal here cannot leave key material behind.
///
/// The staged path is journalled before the member is streamed, like every other
/// member. That matters more here than anywhere else: this member is key
/// material, and it is the one whose leak an operator would not expect beside
/// their store.
pub(crate) fn take_key_member(
    reader: &mut ArchiveReader<std::fs::File>,
    manifest: &BackupManifest,
    header: &MemberHeader,
    dir: &Path,
    journal: &mut Option<StagingJournal>,
) -> Result<ExtractedMember, AmError> {
    if !manifest.keys.carries_key_member() {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "archive member {:?}: the archive carries a key member, but the manifest does \
                 not declare one; refusing an undeclared member",
                header.name
            ),
        ));
    }
    // The manifest records the key member's size, so the member the archive
    // carries must be that size. The check runs before this member is staged,
    // so a member that is not the one the manifest describes never touches
    // disk.
    if let Some(declared) = manifest.keys.bytes
        && header.size != declared
    {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "archive member {:?}: the archive header declares {} bytes, but the \
                 manifest records {} bytes for the key member, so the member cannot be \
                 the one the manifest describes",
                header.name, header.size, declared
            ),
        ));
    }
    let staged = stage_recorded(reader, dir, "restore-key", &header.name, journal)?;
    if !key_head_is_sqlite(&staged)? {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "archive member {:?}: the staged member's first {MAGIC_LEN} bytes are not the \
                 SQLite file magic, so this is not a key-store database",
                header.name
            ),
        ));
    }
    Ok(ExtractedMember {
        member: header.name.clone(),
        file: staged,
    })
}

/// Whether the staged key member starts with the SQLite file magic. The head is
/// read back from the file on disk, not from the reader that wrote it.
///
/// A failure here keeps the code it arrives with, for the same reason staging
/// does: reading the staged key back can fail because the machine could not do
/// it, and calling that a damaged archive sends the operator to the wrong fix.
fn key_head_is_sqlite(staged: &StagedFile) -> Result<bool, AmError> {
    let file = match staged.open() {
        Ok(file) => file,
        Err(error) => {
            return Err(staging_failure(
                KEY_MEMBER,
                "could not be read back",
                &error,
            ));
        }
    };
    let mut head = Vec::with_capacity(MAGIC_LEN);
    if let Err(error) = file.take(MAGIC_LEN as u64).read_to_end(&mut head) {
        let read = staging::io_error("reading the staged key", &error);
        return Err(staging_failure(KEY_MEMBER, "could not be read back", &read));
    }
    Ok(head == SQLITE_MAGIC)
}

#[cfg(test)]
#[path = "key_member_tests.rs"]
mod tests;
