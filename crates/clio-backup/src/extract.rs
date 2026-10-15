// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for taking an archive's declared members back
//! out: proving the payload against the manifest, staging the payload and the
//! key member into the directory the caller names, and refusing an archive
//! that disagrees with its own manifest.
//!
//! ## Owns
//! - `extract_members` and the `ExtractedArchive` / `ExtractedMember` it returns
//! - The member walk and its refusals: a payload whose size or digest does not
//!   match the manifest, and a key member the manifest does not declare or
//!   that is not a SQLite key-store database
//! - Driving the Postgres dump-table walk, with each table member's proof
//!   against the dump meta in `extract_dump_tables`
//! - Removing every staged member that is not returned
//! - Sweeping the last run's staging journal, and recording this run's own
//! - The code a staging failure keeps: archive damage, and an archive over the
//!   reader's own caps, are `ArchiveCorrupt`; a full directory is `QuotaExceeded`
//!
//! ## Does not own
//! - Reading or validating the manifest (see `inventory`); the caller passes the
//!   manifest it already read, so the refusals it decided on and the bytes
//!   staged here come from one read of one file
//! - Container bounds and member streaming (see `container`)
//! - Replacing the live database or snapshotting before a restore (see the
//!   store and CLI halves)
//!
//! ## Boundary
//!
//! Changes to what an extraction accepts or refuses should normally require
//! changes here. Changes to manifest fields should not require changes here.
//!
//! Keep this module focused on extracting an archive's declared members.

use crate::KEY_MEMBER;
use crate::container::{ArchiveReader, MemberHeader};
use crate::dump_meta::{DUMP_TABLES_PREFIX, DumpMeta};
use crate::extract_dump_tables::{
    check_dump_tables_complete, declared_table, read_dump_meta, take_dump_table,
    undeclared_table_member,
};
use crate::key_member::take_key_member;
use crate::manifest::BackupManifest;
use crate::staging::{self, StagedFile};
use crate::staging_journal::{StagingJournal, record_staged, sweep_staging_journals};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// One member taken out of an archive and staged on disk.
#[derive(Debug)]
pub struct ExtractedMember {
    /// The member's name inside the archive.
    pub member: String,
    /// The staged file holding the member's bytes; removes itself on drop.
    pub file: StagedFile,
}

/// The declared members of one archive, staged on disk and ready to be used.
///
/// `payload` and `key` own the staged files, and each staged file removes
/// itself on drop, so a restore that fails after an extraction leaves nothing
/// behind. A killed process runs no drop; the journal is what the next restore
/// sweeps.
#[derive(Debug)]
pub struct ExtractedArchive {
    /// The snapshot id, from the archive's file name; empty when the file name
    /// carries no id.
    pub id: String,
    /// The archive that was extracted.
    pub path: PathBuf,
    /// The manifest that was read and validated before any member came out.
    pub manifest: BackupManifest,
    /// The payload member, staged and proved against the manifest.
    pub payload: ExtractedMember,
    /// The dump's table members, staged and proved against the dump meta;
    /// empty when the archive is not a Postgres dump.
    pub dump_tables: Vec<ExtractedMember>,
    /// The key member, staged and checked to be a SQLite database; `None` when
    /// the manifest declares no key member.
    pub key: Option<ExtractedMember>,
    /// The journal this extraction recorded its staged paths in; dropping this
    /// value drops the journal's file, which is the only reason it is held.
    #[expect(dead_code, reason = "read only by its own drop")]
    journal: Option<StagingJournal>,
}

/// Take the members of one archive out and stage them in `dir`.
///
/// The caller passes the manifest it has already read and validated, and this
/// function uses that same value for every decision below: which member is the
/// payload, the size it must have, and the digest it must match. Re-reading the
/// manifest here would let those decisions and the bytes that are actually
/// staged come from two different reads of one file, so a file replaced between
/// them could be measured against a manifest it no longer agrees with. The walk
/// then proves the payload against that manifest: its declared size before a
/// byte is inflated, and its streamed size and digest after. A key member is
/// staged only when the manifest declares one, and its head must be a SQLite
/// database.
///
/// A Postgres archive carries its rows in `dump/tables/` members that
/// `dump/meta.json` declares. That meta is the payload member, so it is read
/// the moment the payload is taken — before the first table member is opened —
/// and every table member is then refused on its declared size before a byte of
/// it is staged, and proved against the meta's own size and digest after. A
/// member the meta does not declare is skipped, never written out. For any
/// other archive, a member the manifest does not describe is skipped.
///
/// Every staged file that is not returned is removed: a member that fails a
/// check drops its staged file, and the hand-removal covers a stream that
/// fails before the file is adopted. The journal records each staged path
/// before its member is streamed, and is swept here, before this run stages
/// anything: a killed process runs no drop, so a staged file can outlive the
/// process that made it, and only a later run's sweep can remove it.
///
/// # Errors
/// The archive is missing, unreadable, truncated, or carries a member this
/// build refuses; a member that disagrees with the manifest; a declared member
/// the archive does not carry; or a staged file that cannot be created.
pub fn extract_members(
    archive: &Path,
    manifest: &BackupManifest,
    dir: &Path,
) -> Result<ExtractedArchive, AmError> {
    sweep_staging_journals(dir);
    let mut journal: Option<StagingJournal> = None;
    let file = match std::fs::File::open(archive) {
        Ok(file) => file,
        Err(error) => {
            let context = format!("opening {}", archive.display());
            return Err(staging::io_error(&context, &error));
        }
    };
    let mut reader = ArchiveReader::new(file)?;
    let mut payload: Option<ExtractedMember> = None;
    let mut key: Option<ExtractedMember> = None;
    // A Postgres dump's table members are declared by `dump/meta.json`, not by
    // the manifest. That meta rides in the payload member, which is the second
    // member of the archive, so it is read as soon as the payload is taken: a
    // table member is never opened, let alone staged, before the archive has
    // said what that member should be.
    let postgres = manifest.store.backend == "postgres";
    let mut meta: Option<DumpMeta> = None;
    let mut dump_tables: Vec<ExtractedMember> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    while let Some(header) = reader.next_member()? {
        if header.name == manifest.payload.member {
            let taken = take_payload(&mut reader, manifest, &header, dir, &mut journal)?;
            if postgres {
                meta = Some(read_dump_meta(&taken, manifest)?);
            }
            payload = Some(taken);
        } else if header.name == KEY_MEMBER {
            key = Some(take_key_member(
                &mut reader,
                manifest,
                &header,
                dir,
                &mut journal,
            )?);
        } else if postgres && header.name.starts_with(DUMP_TABLES_PREFIX) {
            // Refused before anything is staged: a table member the meta does
            // not declare has no size and no digest to prove, so it is never
            // written to disk, not even to be thrown away afterwards.
            let table = declared_table(meta.as_ref(), &header.name)
                .ok_or_else(|| undeclared_table_member(&header.name))?;
            let taken = take_dump_table(&mut reader, &header, table, dir, &mut journal)?;
            dump_tables.push(taken);
            names.push(header.name.clone());
        } else if let Err(error) = reader.skip_current() {
            return Err(corrupt(&header.name, &error));
        }
    }
    let payload = payload.ok_or_else(|| missing_payload(manifest))?;
    if manifest.keys.carries_key_member() && key.is_none() {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "the manifest declares key member {KEY_MEMBER:?}, but the archive does not carry it"
            ),
        ));
    }
    if postgres {
        // The payload branch is the only place the meta can be read, and it
        // reads it for every Postgres archive, so by here it is always present.
        let meta = meta.expect("a Postgres archive's payload member is its dump meta");
        check_dump_tables_complete(&meta, &names)?;
    }
    Ok(ExtractedArchive {
        id: crate::archive_name::snapshot_id_of(archive).unwrap_or_default(),
        path: archive.to_path_buf(),
        manifest: manifest.clone(),
        payload,
        dump_tables,
        key,
        journal,
    })
}

/// Take the open payload member: refuse a header size the manifest does not
/// record before inflating anything, then prove the streamed size and digest.
fn take_payload(
    reader: &mut ArchiveReader<std::fs::File>,
    manifest: &BackupManifest,
    header: &MemberHeader,
    dir: &Path,
    journal: &mut Option<StagingJournal>,
) -> Result<ExtractedMember, AmError> {
    if header.size != manifest.payload.bytes {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "archive member {:?}: the archive header declares {} bytes, but the manifest \
                 records {} bytes for the payload, so the member cannot be the one the manifest \
                 describes",
                header.name, header.size, manifest.payload.bytes
            ),
        ));
    }
    let staged = stage_recorded(reader, dir, "restore-payload", &header.name, journal)?;
    let mut problems: Vec<&str> = Vec::new();
    if staged.bytes() != manifest.payload.bytes {
        problems.push("size");
    }
    if staged.sha256() != manifest.payload.sha256 {
        problems.push("digest");
    }
    if !problems.is_empty() {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "archive member {:?}: the payload's {} {} not match the manifest's record \
                 (manifest: {} bytes, digest {}; member: {} bytes, digest {})",
                header.name,
                problems.join(" and the "),
                if problems.len() == 1 { "does" } else { "do" },
                manifest.payload.bytes,
                manifest.payload.sha256,
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

/// Stream the open member into an owner-only staged file in `dir`, record the
/// staged path in `journal` before the member is streamed, and remove the file
/// on every path that does not adopt it.
///
/// Recording comes before the stream, not after it, because a process killed
/// part-way through a member is exactly the case the journal exists for: an
/// entry written afterwards would never be written at all. That is why this is
/// the one staging path every member goes through rather than something the
/// callers opt into.
///
/// An unarmed journal is not a refusal; a failed record is one, and takes the
/// staged file with it: no journal names it, so no sweep could ever remove it.
pub(crate) fn stage_recorded(
    reader: &mut ArchiveReader<std::fs::File>,
    dir: &Path,
    tag: &str,
    member: &str,
    journal: &mut Option<StagingJournal>,
) -> Result<StagedFile, AmError> {
    let (path, mut file) = match staging::begin_staged(dir, tag) {
        Ok(staged) => staged,
        // The archive may be perfectly intact here: the directory can be full,
        // unwritable, or out of names. Keeping the filesystem's own code is what
        // stops an out-of-space condition being reported as a damaged archive.
        Err(error) => return Err(staging_failure(member, "could not be staged", &error)),
    };
    if let Err(error) = record_staged(journal, dir, &path) {
        let _ = std::fs::remove_file(&path);
        return Err(staging_failure(member, "could not be journalled", &error));
    }
    let digest = match reader.read_current(&mut file) {
        Ok(digest) => digest,
        Err(error) => {
            // The staged file is not adopted, so nothing would remove it.
            let _ = std::fs::remove_file(&path);
            return Err(staging_failure(member, "could not be staged", &error));
        }
    };
    if let Err(error) = file.sync_all() {
        let flush = staging::io_error("flushing the member", &error);
        let _ = std::fs::remove_file(&path);
        return Err(staging_failure(member, "could not be staged", &flush));
    }
    Ok(staging::adopt_staged(path, digest.size, digest.sha256))
}

/// The refusal for a payload member the manifest declares and the archive does
/// not carry.
fn missing_payload(manifest: &BackupManifest) -> AmError {
    AmError::new(
        ErrorCode::ArchiveCorrupt,
        format!(
            "the manifest declares payload member {:?}, but the archive does not carry it",
            manifest.payload.member
        ),
    )
}

/// Re-label a container error as archive corruption, naming the member.
pub(crate) fn corrupt(member: &str, error: &AmError) -> AmError {
    AmError::new(
        ErrorCode::ArchiveCorrupt,
        format!("archive member {member:?}: {}", error.message),
    )
}

/// Re-label a failure that happened while staging a member, naming the member
/// and what was being done to it, and keeping the code the failure already
/// carries.
///
/// A damaged archive arrives as `ArchiveCorrupt` and stays that way: the
/// container decides a short stream, a bad padding run, a bad gzip member, and
/// its own size cap before it reaches this point, so its code is already right.
/// A directory that is full or over its quota arrives as `QuotaExceeded` and
/// stays that way: that one is the machine's condition and not the archive's,
/// which is why the two codes are kept apart. Re-labelling every failure as
/// damage is what made a full volume tell the operator their archive was broken.
pub(crate) fn staging_failure(member: &str, did: &str, error: &AmError) -> AmError {
    AmError::new(
        error.code,
        format!("archive member {member:?} {did}: {}", error.message),
    )
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;

/// The staging-directory refusals, which are the machine's fault.
#[cfg(test)]
#[path = "extract_staging_tests.rs"]
mod staging_tests;

#[cfg(test)]
#[path = "extract_restore_readiness_tests.rs"]
mod extract_restore_readiness_tests;
