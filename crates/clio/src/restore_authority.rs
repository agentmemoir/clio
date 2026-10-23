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
//!
//! ## Boundary
//!
//! Changes to how an archive's members become authority-check inputs should
//! normally require changes here. Changes to the authority rules should not.
//!
//! Keep this module focused on classifying one archive against its manifest.

use clio_backup::{BackupManifest, ExtractedArchive, ExtractedMember};
use clio_compliance::{ArchiveClaim, MemberFacts, tombstone_salt, tombstone_salt_fingerprint};
use clio_store::sha256_hex;
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The Postgres COPY text reader, split out so this file stays focused on
/// classifying the archive.
#[path = "restore_authority_copy.rs"]
mod restore_authority_copy;

/// The payload table holding one-way subject hashes.
const TOMBSTONE_TABLE: &str = "compliance_tombstones";

/// The payload table linking a subject id to its data key.
const SUBJECT_KEYS_TABLE: &str = "subject_keys";

/// The payload table holding one row per item, each row keeping its subject
/// link after the subject's key was destroyed.
const ITEMS_TABLE: &str = "items";

/// The key-store table holding one row per subject key slot.
const KEY_SLOTS_TABLE: &str = "slots";

/// The key-store column holding the sealed DEK; `NULL` once it is destroyed.
const KEY_COLUMN: &str = "key_b64";

/// The tombstone column whose value is already a salted subject hash.
const TOMBSTONE_HASH_COLUMN: &str = "subject_hash";

/// The subject id columns, which are plaintext ids that must be hashed.
const SUBJECT_ID_COLUMN: &str = "subject_id";

/// One archive's claimed authority and the membership fact of every member.
#[derive(Debug)]
pub struct ClassifiedArchive {
    /// The authority the archive's manifest records.
    pub claim: ArchiveClaim,
    /// What every archive member shows about subject references.
    pub members: Vec<MemberFacts>,
}

/// Why an archive could not be classified for the authority check.
#[derive(Debug)]
pub enum ClassifyRefusal {
    /// The manifest records no authority, so the archive fails closed.
    ArchiveMissingAuthority,
    /// The archive's salt fingerprint is absent or differs from the current
    /// deployment salt, so its stored subject ids cannot be re-hashed.
    SaltFingerprintMismatch,
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
    /// authority. A member that cannot be classified is
    /// [`ErrorCode::ConfigCorrupt`], the same code the check gives an unknown
    /// member. A read failure keeps the code it carries.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::ArchiveMissingAuthority => ErrorCode::NotFound,
            Self::SaltFingerprintMismatch => ErrorCode::Forbidden,
            Self::UnknownMember(_) => ErrorCode::ConfigCorrupt,
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
/// - a SQLite payload yields a reference for every `compliance_tombstones`
///   hash and every hashed `subject_keys` and `items` subject id;
/// - a Postgres payload is the dump meta, and its `dump/tables` members yield a
///   reference for the subject columns of the three subject tables;
/// - the key member yields a reference for every hashed `slots` subject id;
/// - any other member is [`MemberFacts::Unknown`] and is never skipped.
///
/// A reference is [`MemberFacts::LiveKey`] when the archive's own key store
/// still holds a usable key for that subject, because restoring it would make
/// the subject's content readable. Every other reference is the content-free
/// [`MemberFacts::Reference`]: a destroyed key slot, a tombstone, or a
/// ciphertext row the archive carries no usable key for. That distinction is
/// what lets a backup taken after an erasure be restored while a pre-erasure
/// archive that still carries the subject's key still refuses.
///
/// # Errors
/// The manifest records no authority ([`ClassifyRefusal::ArchiveMissingAuthority`]),
/// the salt fingerprint is absent or differs ([`ClassifyRefusal::SaltFingerprintMismatch`]),
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
                members.extend(classify_sqlite_payload(extracted.payload.file.path(), &live)?);
            }
        } else if manifest.keys.carries_key_member()
            && manifest.keys.member.as_deref() == Some(name.as_str())
        {
            members.extend(classify_key_store(extracted.key.as_ref(), &live)?);
        } else if let Some(table) = meta.as_ref().and_then(|meta| table_named(meta, &name)) {
            members.extend(classify_dump_table(table, &extracted, &live)?);
        } else {
            members.push(MemberFacts::Unknown);
        }
    }
    Ok(ClassifiedArchive { claim, members })
}

/// The dump table the dump meta declares under `member`, if any.
fn table_named<'a>(
    meta: &'a clio_backup::dump_meta::DumpMeta,
    member: &str,
) -> Option<&'a clio_backup::dump_meta::DumpTable> {
    meta.tables.iter().find(|table| table.member == member)
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

/// Parse the staged payload as a Postgres dump meta.
fn read_dump_meta(payload: &Path) -> Result<clio_backup::dump_meta::DumpMeta, ClassifyRefusal> {
    let bytes = std::fs::read(payload)
        .map_err(|error| io_error("reading the dump meta", payload, &error))?;
    Ok(clio_backup::dump_meta::DumpMeta::from_json_bytes(&bytes)?)
}

/// Classify a SQLite payload: its tombstones, subject keys, and item subjects.
///
/// An item row keeps its subject link after the subject's key was destroyed,
/// so the item's subject id is a presence the archive carries.
fn classify_sqlite_payload(
    payload: &Path,
    live: &BTreeSet<String>,
) -> Result<Vec<MemberFacts>, ClassifyRefusal> {
    let mut hashes = BTreeSet::new();
    for hash in sqlite_column(payload, TOMBSTONE_TABLE, TOMBSTONE_HASH_COLUMN)? {
        if !hash.is_empty() {
            hashes.insert(hash);
        }
    }
    let salt = tombstone_salt();
    for id in sqlite_column(payload, SUBJECT_KEYS_TABLE, SUBJECT_ID_COLUMN)? {
        hashes.insert(sha256_hex(&salt, &id));
    }
    for id in sqlite_column(payload, ITEMS_TABLE, SUBJECT_ID_COLUMN)? {
        hashes.insert(sha256_hex(&salt, &id));
    }
    Ok(facts_from_hashes(hashes, live))
}

/// The subjects whose key slot in the archive's key store still carries a
/// usable key, as deployment-wide subject hashes.
///
/// A slot whose key column is unreadable is treated as live, so an archive that
/// cannot be shown to have destroyed the key fails closed.
fn live_key_hashes(key: Option<&ExtractedMember>) -> Result<BTreeSet<String>, ClassifyRefusal> {
    let Some(key) = key else {
        return Ok(BTreeSet::new());
    };
    let path = key.file.path();
    let ids = if column_present(path, KEY_SLOTS_TABLE, KEY_COLUMN)? {
        sqlite_column_where(path, KEY_SLOTS_TABLE, SUBJECT_ID_COLUMN, "key_b64 IS NOT NULL")?
    } else {
        sqlite_column(path, KEY_SLOTS_TABLE, SUBJECT_ID_COLUMN)?
    };
    let salt = tombstone_salt();
    Ok(ids
        .into_iter()
        .map(|id| sha256_hex(&salt, &id))
        .collect())
}

/// Classify the archive's key member: one fact per key slot id.
fn classify_key_store(
    key: Option<&ExtractedMember>,
    live: &BTreeSet<String>,
) -> Result<Vec<MemberFacts>, ClassifyRefusal> {
    let Some(key) = key else {
        return Ok(vec![MemberFacts::None]);
    };
    let salt = tombstone_salt();
    let mut hashes = BTreeSet::new();
    for id in sqlite_column(key.file.path(), KEY_SLOTS_TABLE, SUBJECT_ID_COLUMN)? {
        hashes.insert(sha256_hex(&salt, &id));
    }
    Ok(facts_from_hashes(hashes, live))
}

/// Classify one Postgres dump table member from its `COPY` text.
///
/// Only the three subject tables carry a subject reference; every other table
/// is content and proves no content-free subject record. The item table is one
/// of them: an item row keeps its subject link after the subject's key was
/// destroyed, so the archive names the subject. A subject table whose subject
/// column is absent cannot be classified, so the archive is refused rather than
/// passing one whose membership is unknown.
fn classify_dump_table(
    table: &clio_backup::dump_meta::DumpTable,
    extracted: &ExtractedArchive,
    live: &BTreeSet<String>,
) -> Result<Vec<MemberFacts>, ClassifyRefusal> {
    let column = match table.table.as_str() {
        TOMBSTONE_TABLE => Some((TOMBSTONE_HASH_COLUMN, false)),
        SUBJECT_KEYS_TABLE | ITEMS_TABLE => Some((SUBJECT_ID_COLUMN, true)),
        _ => None,
    };
    let Some((column, hash_the_id)) = column else {
        return Ok(vec![MemberFacts::None]);
    };
    let Some(index) = table.columns.iter().position(|entry| entry.name == column) else {
        return Err(ClassifyRefusal::UnknownMember(table.member.clone()));
    };
    let Some(member) = extracted
        .dump_tables
        .iter()
        .find(|member| member.member == table.member)
    else {
        return Err(ClassifyRefusal::UnknownMember(table.member.clone()));
    };
    let file = member.file.open()?;
    let salt = tombstone_salt();
    let mut hashes = BTreeSet::new();
    for value in restore_authority_copy::copy_column(std::io::BufReader::new(file), index)? {
        if hash_the_id {
            hashes.insert(sha256_hex(&salt, &value));
        } else {
            hashes.insert(value);
        }
    }
    Ok(facts_from_hashes(hashes, live))
}

/// The distinct subject references, or one `None` when there are none.
///
/// A hash the archive's key store still holds a usable key for is a
/// [`MemberFacts::LiveKey`]; every other one is the content-free
/// [`MemberFacts::Reference`].
fn facts_from_hashes(hashes: BTreeSet<String>, live: &BTreeSet<String>) -> Vec<MemberFacts> {
    if hashes.is_empty() {
        vec![MemberFacts::None]
    } else {
        hashes
            .into_iter()
            .map(|subject_hash| {
                if live.contains(&subject_hash) {
                    MemberFacts::LiveKey { subject_hash }
                } else {
                    MemberFacts::Reference { subject_hash }
                }
            })
            .collect()
    }
}

/// Every value of `column` in `table`, read from the SQLite database at `path`.
///
/// A missing table or column yields no values: the store simply holds no such
/// record. A NULL value is skipped: it names no subject, which is what the
/// `COPY` text reader does with its `\N` marker. The connection is opened
/// read-write with `query_only` set, the pattern that lets a store recover a
/// hot journal while a statement still cannot write.
fn sqlite_column(path: &Path, table: &str, column: &str) -> Result<Vec<String>, ClassifyRefusal> {
    sqlite_column_where(path, table, column, "")
}

/// Every value of `column` in the rows of `table` matching `predicate`.
///
/// `predicate` is a constant SQL expression owned by this module; it never
/// carries caller input.
fn sqlite_column_where(
    path: &Path,
    table: &str,
    column: &str,
    predicate: &str,
) -> Result<Vec<String>, ClassifyRefusal> {
    let connection =
        rusqlite::Connection::open(path).map_err(|error| sqlite_error(path, table, &error))?;
    connection
        .execute_batch("PRAGMA query_only = ON")
        .map_err(|error| sqlite_error(path, table, &error))?;
    if !column_exists(&connection, table, column)
        .map_err(|error| sqlite_error(path, table, &error))?
    {
        return Ok(Vec::new());
    }
    let sql = if predicate.is_empty() {
        format!("SELECT {column} FROM {table}")
    } else {
        format!("SELECT {column} FROM {table} WHERE {predicate}")
    };
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| sqlite_error(path, table, &error))?;
    let rows = statement
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .map_err(|error| sqlite_error(path, table, &error))?;
    let mut values = Vec::new();
    for row in rows {
        if let Some(value) = row.map_err(|error| sqlite_error(path, table, &error))? {
            values.push(value);
        }
    }
    Ok(values)
}

/// Whether `table` holds a column named `column` in the database at `path`.
fn column_present(path: &Path, table: &str, column: &str) -> Result<bool, ClassifyRefusal> {
    let connection =
        rusqlite::Connection::open(path).map_err(|error| sqlite_error(path, table, &error))?;
    connection
        .execute_batch("PRAGMA query_only = ON")
        .map_err(|error| sqlite_error(path, table, &error))?;
    column_exists(&connection, table, column)
        .map_err(|error| sqlite_error(path, table, &error))
}

/// Whether `table` holds a column named `column`.
fn column_exists(
    connection: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        rusqlite::params![table, column],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// The refusal for a filesystem failure, naming the path.
fn io_error(context: &str, path: &Path, error: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("{context} {}: {error}", path.display()),
    )
}

/// The refusal for a SQLite read failure, naming the table it was reading.
fn sqlite_error(path: &Path, table: &str, error: &rusqlite::Error) -> ClassifyRefusal {
    ClassifyRefusal::Io(AmError::new(
        ErrorCode::Internal,
        format!("reading table {table:?} from {}: {error}", path.display()),
    ))
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
#[path = "restore_authority_tests.rs"]
mod restore_authority_tests;
