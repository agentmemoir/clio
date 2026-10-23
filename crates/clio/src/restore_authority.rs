// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for turning one backup archive into the two
//! inputs the guarded-restore authority check consumes: the authority the
//! archive claims it was made under, and one membership fact per archive
//! member.
//!
//! ## Owns
//! - `classify_archive`, `ClassifiedArchive`, and `ClassifyRefusal`
//! - Extracting an archive into a private scratch directory, then reading the
//!   subject references its payload, key store, and dump tables carry
//! - Marking a subject whose archive key slot still holds a usable key as
//!   `MemberFacts::LiveKey`, and every other reference as the content-free
//!   `MemberFacts::Reference`
//! - Marking a member it cannot recognize as `MemberFacts::Unknown`
//!
//! ## Does not own
//! - The authority check itself (`clio-compliance` owns `check_archive_authority`)
//! - Where a restore places bytes (the restore flow and the store own that)
//! - The archive container, manifest, or extraction (`clio-backup` owns those)
//! - The SQLite payload and key-store reads (`restore_authority_sqlite` owns
//!   those)
//! - The Postgres dump-meta and dump-table reads (`restore_authority_dump` owns
//!   those)
//!
//! ## Boundary
//!
//! Changes to how an archive's members become authority-check inputs should
//! normally require changes here. Changes to the authority rules should not.
//!
//! Keep this module focused on classifying one archive against its manifest.

use clio_backup::{BackupManifest, ExtractedMember};
use clio_compliance::{ArchiveClaim, MemberFacts, sha256_hex_bytes, tombstone_salt_fingerprint};
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The Postgres COPY text reader, split out so this file stays focused on
/// classifying the archive.
#[path = "restore_authority_copy.rs"]
mod restore_authority_copy;

/// The SQLite payload and key-store reads.
#[path = "restore_authority_sqlite.rs"]
mod restore_authority_sqlite;

/// The Postgres dump-meta and dump-table reads.
#[path = "restore_authority_dump.rs"]
mod restore_authority_dump;

use restore_authority_dump::{classify_dump_table, read_dump_meta, table_named};
use restore_authority_sqlite::{classify_key_store, classify_sqlite_payload, live_key_hashes};

/// The payload table holding one-way subject hashes.
pub(super) const TOMBSTONE_TABLE: &str = "compliance_tombstones";

/// The payload table linking a subject id to its data key.
pub(super) const SUBJECT_KEYS_TABLE: &str = "subject_keys";

/// The payload table holding one row per item, each row keeping its subject
/// link after the subject's key was destroyed.
pub(super) const ITEMS_TABLE: &str = "items";

/// The key-store table holding one row per subject key slot.
pub(super) const KEY_SLOTS_TABLE: &str = "slots";

/// The key-store column holding the sealed DEK; `NULL` once it is destroyed.
pub(super) const KEY_COLUMN: &str = "key_b64";

/// The tombstone column whose value is already a salted subject hash.
pub(super) const TOMBSTONE_HASH_COLUMN: &str = "subject_hash";

/// The subject id columns, which are plaintext ids that must be hashed.
pub(super) const SUBJECT_ID_COLUMN: &str = "subject_id";

/// One archive's claimed authority and the membership fact of every member.
#[derive(Debug)]
pub struct ClassifiedArchive {
    /// The authority the archive's manifest records.
    pub claim: ArchiveClaim,
    /// What every archive member shows about subject references.
    pub members: Vec<MemberFacts>,
    /// The SHA-256 of the key member the members were classified from, or
    /// `None` when the archive carries none.
    ///
    /// The manifest records only a size for this member, so the plan re-extracts
    /// the archive and must prove that the key member it will place is the one
    /// that was classified. A mismatch means the archive changed between the
    /// authority check and placement and is refused.
    pub key_member_sha256: Option<String>,
}

impl ClassifiedArchive {
    /// Prove that the key member extracted for placement is the one these
    /// members were classified from.
    ///
    /// # Errors
    /// [`ClassifyRefusal::KeyMemberChanged`] when a member classified as
    /// carrying no usable key for a denied subject is replaced by a different
    /// one before placement, or the reverse.
    pub fn verify_key_member(&self, key: Option<&ExtractedMember>) -> Result<(), ClassifyRefusal> {
        let digest = key
            .map(|member| file_sha256(member.file.path()))
            .transpose()?;
        if digest == self.key_member_sha256 {
            Ok(())
        } else {
            Err(ClassifyRefusal::KeyMemberChanged)
        }
    }
}

/// Why an archive could not be classified for the authority check.
#[derive(Debug)]
pub enum ClassifyRefusal {
    /// The manifest records no authority, so the archive fails closed.
    ArchiveMissingAuthority,
    /// The archive's salt fingerprint is absent or differs from the current
    /// deployment salt, so its stored subject ids cannot be re-hashed.
    SaltFingerprintMismatch,
    /// The archive's key member is not a key store this build can read, so it
    /// cannot be shown to carry no usable key.
    KeyStoreUnreadable,
    /// The key member extracted for placement differs from the one the
    /// authority check classified.
    KeyMemberChanged,
    /// A member whose presence was expected could not be classified.
    UnknownMember(String),
    /// Reading or extracting the archive failed.
    Io(AmError),
}

impl ClassifyRefusal {
    /// The refusal's stable code in the crate's error vocabulary.
    ///
    /// A missing authority is [`ErrorCode::NotFound`], the same code the
    /// authority check gives a missing presented authority. A salt mismatch is
    /// [`ErrorCode::Forbidden`], the same code the check gives a mismatched
    /// authority. An unreadable key store and a member that cannot be
    /// classified are [`ErrorCode::ConfigCorrupt`], the same code the check
    /// gives an unknown member. A key member that changed is
    /// [`ErrorCode::ArchiveCorrupt`], because the archive is no longer the one
    /// the check read. A read failure keeps the code it carries.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::ArchiveMissingAuthority => ErrorCode::NotFound,
            Self::SaltFingerprintMismatch => ErrorCode::Forbidden,
            Self::KeyStoreUnreadable | Self::UnknownMember(_) => ErrorCode::ConfigCorrupt,
            Self::KeyMemberChanged => ErrorCode::ArchiveCorrupt,
            Self::Io(error) => error.code,
        }
    }
}

impl std::fmt::Display for ClassifyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArchiveMissingAuthority => {
                f.write_str("the archive records no erasure authority")
            }
            Self::SaltFingerprintMismatch => f.write_str(
                "the archive's subject-salt fingerprint does not match the current deployment salt",
            ),
            Self::KeyStoreUnreadable => {
                f.write_str("the archive's key member is not a readable key store")
            }
            Self::KeyMemberChanged => {
                f.write_str("the archive's key member changed after its authority was checked")
            }
            Self::UnknownMember(member) => {
                write!(f, "the archive member {member:?} cannot be classified")
            }
            Self::Io(error) => write!(f, "the archive could not be classified: {}", error.message),
        }
    }
}

impl std::error::Error for ClassifyRefusal {}

impl From<AmError> for ClassifyRefusal {
    fn from(error: AmError) -> Self {
        Self::Io(error)
    }
}

/// Classify one archive against the manifest read from it.
///
/// The claim comes from `manifest.authority`; an archive that records none is
/// refused before anything is read, because the authority check has nothing to
/// compare. An archive whose recorded salt fingerprint is absent or differs
/// from the current deployment salt is also refused: its plaintext subject ids
/// would be re-hashed under a different salt than the one its records were made
/// with. The declared members are then extracted into a private scratch
/// directory and every member is classified:
///
/// - the manifest member proves no subject reference;
/// - a SQLite payload names every `compliance_tombstones` hash and every
///   hashed `subject_keys` and `items` subject id;
/// - a Postgres payload is the dump meta, and its `dump/tables` members name
///   the subject columns of the three subject tables;
/// - the key member names every hashed `slots` subject id;
/// - any other member is [`MemberFacts::Unknown`] and is never skipped.
///
/// A reference is [`MemberFacts::LiveKey`] when the archive's own key store
/// still holds a usable key for that subject, because restoring it would make
/// the subject's content readable. Every other reference is the content-free
/// [`MemberFacts::Reference`]: a destroyed key slot, a tombstone, or a
/// ciphertext row the archive carries no usable key for. An archive with no key
/// member leaves the target's key store in place, so none of its references can
/// be shown content-free and all of them are [`MemberFacts::UnprovenKey`]. That
/// distinction is what lets a backup taken after an erasure be restored while a
/// pre-erasure archive that still carries the subject's key still refuses.
///
/// # Errors
/// The manifest records no authority ([`ClassifyRefusal::ArchiveMissingAuthority`]),
/// the salt fingerprint is absent or differs ([`ClassifyRefusal::SaltFingerprintMismatch`]),
/// the key member is not a readable key store ([`ClassifyRefusal::KeyStoreUnreadable`]),
/// an expected member cannot be classified ([`ClassifyRefusal::UnknownMember`]),
/// or the archive cannot be read ([`ClassifyRefusal::Io`]).
pub fn classify_archive(
    archive: &Path,
    manifest: &BackupManifest,
) -> Result<ClassifiedArchive, ClassifyRefusal> {
    let authority = manifest
        .authority
        .as_ref()
        .ok_or(ClassifyRefusal::ArchiveMissingAuthority)?;
    // The archive's subject ids are re-hashed with the deploying salt below, so
    // an archive made under a different salt cannot be compared. An absent
    // fingerprint fails closed.
    if authority.salt_fingerprint.as_deref() != Some(tombstone_salt_fingerprint().as_str()) {
        return Err(ClassifyRefusal::SaltFingerprintMismatch);
    }
    let claim = ArchiveClaim {
        deployment_id: authority.deployment_id.clone(),
        generation: authority.generation,
        salt_fingerprint: authority.salt_fingerprint.clone(),
    };

    let scratch = Scratch::new()?;
    let extracted = clio_backup::extract_members(archive, manifest, scratch.path())?;
    let meta = if manifest.store.backend == "sqlite" {
        None
    } else {
        Some(read_dump_meta(extracted.payload.file.path())?)
    };
    // The subjects whose key slot in the archive still holds a usable key. A
    // reference to one of them is not content-free: restoring the archive would
    // make it readable.
    let live = live_key_hashes(extracted.key.as_ref())?;
    // An archive that carries no key member leaves the target's key store in
    // place, so this build cannot show that a reference is content-free: every
    // reference is treated as usable key material.
    let keys_present = extracted.key.is_some();

    // The manifest records only a size for the key member, so the digest is
    // captured here and re-proved against the plan's own extraction before
    // placement.
    let key_member_sha256 = extracted
        .key
        .as_ref()
        .map(|member| file_sha256(member.file.path()))
        .transpose()?;

    let mut members = Vec::new();
    for name in member_names(archive)? {
        if name == clio_backup::MANIFEST_MEMBER {
            members.push(MemberFacts::None);
        } else if name == manifest.payload.member {
            if meta.is_some() {
                // The Postgres payload is the dump meta: it describes tables
                // and never carries a subject reference itself.
                members.push(MemberFacts::None);
            } else {
                members.extend(classify_sqlite_payload(
                    extracted.payload.file.path(),
                    &live,
                    keys_present,
                )?);
            }
        } else if manifest.keys.carries_key_member()
            && manifest.keys.member.as_deref() == Some(name.as_str())
        {
            members.extend(classify_key_store(extracted.key.as_ref(), &live)?);
        } else if let Some(table) = meta.as_ref().and_then(|meta| table_named(meta, &name)) {
            members.extend(classify_dump_table(table, &extracted, &live, keys_present)?);
        } else {
            members.push(MemberFacts::Unknown);
        }
    }
    Ok(ClassifiedArchive {
        claim,
        members,
        key_member_sha256,
    })
}

/// Every member name the archive carries, in archive order.
fn member_names(archive: &Path) -> Result<Vec<String>, ClassifyRefusal> {
    let file = std::fs::File::open(archive)
        .map_err(|error| io_error("opening the archive", archive, &error))?;
    let mut reader = clio_backup::ArchiveReader::new(file)?;
    let mut names = Vec::new();
    while let Some(header) = reader.next_member()? {
        names.push(header.name);
        reader.skip_current()?;
    }
    Ok(names)
}

/// The distinct subject references, or one `None` when there are none.
///
/// Without a key member the restore leaves the target's key store in place, so
/// no reference can be shown content-free: every one is
/// [`MemberFacts::UnprovenKey`]. With a key member, a hash the archive still
/// holds a usable key for is a [`MemberFacts::LiveKey`] and every other one is
/// the content-free [`MemberFacts::Reference`].
pub(super) fn facts_from_hashes(
    hashes: BTreeSet<String>,
    live: &BTreeSet<String>,
    keys_present: bool,
) -> Vec<MemberFacts> {
    if hashes.is_empty() {
        vec![MemberFacts::None]
    } else {
        hashes
            .into_iter()
            .map(|subject_hash| {
                if !keys_present {
                    MemberFacts::UnprovenKey { subject_hash }
                } else if live.contains(&subject_hash) {
                    MemberFacts::LiveKey { subject_hash }
                } else {
                    MemberFacts::Reference { subject_hash }
                }
            })
            .collect()
    }
}

/// The refusal for a filesystem failure, naming the path.
pub(super) fn io_error(context: &str, path: &Path, error: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("{context} {}: {error}", path.display()),
    )
}

/// The SHA-256 of a file's bytes.
fn file_sha256(path: &Path) -> Result<String, ClassifyRefusal> {
    let bytes = std::fs::read(path).map_err(|error| io_error("reading a member", path, &error))?;
    Ok(sha256_hex_bytes(&bytes))
}

/// A private directory for one classification, removed when it drops.
struct Scratch(PathBuf);

impl Scratch {
    /// Create a unique scratch directory under the system temporary directory.
    fn new() -> Result<Self, ClassifyRefusal> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| {
                ClassifyRefusal::Io(AmError::new(
                    ErrorCode::Internal,
                    format!("reading the clock: {error}"),
                ))
            })?
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "clio-restore-authority-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)
            .map_err(|error| io_error("creating the scratch directory", &path, &error))?;
        Ok(Self(path))
    }

    /// Where the staged members live.
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
#[path = "restore_authority_test_support.rs"]
mod restore_authority_test_support;

#[cfg(test)]
#[path = "restore_authority_tests.rs"]
mod restore_authority_tests;

/// The member-fact cases, split out so the refusal cases stay focused.
#[cfg(test)]
#[path = "restore_authority_payload_tests.rs"]
mod restore_authority_payload_tests;
