// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the key-store member of a snapshot: finding
//! the local dev-KMS key store for a resolved database target and reporting
//! how many subjects it holds.
//!
//! ## Owns
//! - `key_member_source`: the key-store file a snapshot should carry, plus
//!   its subject count
//! - The refusal for a key-store path that is not a SQLite key-store database
//!
//! ## Does not own
//! - Placing the member in an archive (see `clio-backup`, which copies the
//!   bytes verbatim)
//! - The key store's schema, format-version gate, or row operations (see
//!   `local_kms_db`)
//! - Choosing the key-store path (see `clio-config`'s `kms_path`)
//!
//! ## Boundary
//!
//! Changes to how a snapshot learns about the key store should normally
//! require changes here. Changes to the archive layout should not.
//!
//! Keep this module focused on the snapshot's key-store member.

use crate::local_kms_db::{KmsDb, SQLITE_MAGIC};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// Bytes read to recognise a SQLite key-store database.
const MAGIC_LEN: usize = 16;

/// The key store a snapshot should carry, and how many subjects it holds.
///
/// `subjects` is a count and nothing else: no subject identifier, no key
/// byte, and no key length leaves this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMemberSource {
    /// Path of the key-store file to copy verbatim.
    pub path: PathBuf,
    /// How many data-subject slots the key store holds. The reserved lexical
    /// blind-index slot is key infrastructure, not a data subject, and is not
    /// counted.
    pub subjects: u64,
}

/// Resolve the key-store member for one database target.
///
/// `kms_path` is the already-resolved key-store path (`clio-config`'s
/// `resolve_kms_path`); `None` means the target keeps its keys in process
/// memory, so there is nothing to copy and the caller must report the
/// snapshot as keyless rather than claiming keys it did not capture.
///
/// A missing file is `None` for the same reason: a store nothing has been
/// written to has no key store yet, and a snapshot must not invent one.
///
/// A path that exists but is not a regular file is refused rather than reported
/// as absent. The operator has something on the key-store path, so reporting
/// "no key store" would write an archive that claims its keys are in process
/// memory while their key store sits on disk unread — the one outcome the
/// archive half is forbidden to produce silently.
///
/// The count is read through the store's own accessor, so the key store is
/// never opened a second way, and the file is only ever read — a backup does
/// not migrate, seed, or tighten anything.
///
/// # Errors
/// The key store exists but is unreadable, is not a regular file, is not a
/// SQLite key-store database, or reports an unsupported format version.
pub fn key_member_source(kms_path: Option<&Path>) -> Result<Option<KeyMemberSource>, AmError> {
    let Some(path) = kms_path else {
        return Ok(None);
    };
    if !key_store_is_present(path)? {
        return Ok(None);
    }
    refuse_non_sqlite_store(path)?;
    let mut store = KmsDb::open(path.to_path_buf())?;
    let subjects = store.subject_count()?;
    Ok(Some(KeyMemberSource {
        path: path.to_path_buf(),
        subjects,
    }))
}

/// Tell an absent key store from one that is present but cannot be carried.
///
/// `metadata` follows symlinks, so a dangling symlink is still "absent" and
/// keeps its existing meaning; anything the filesystem can see that is not a
/// regular file is a refusal, because the operator put it there.
///
/// # Errors
/// The path cannot be examined, or it exists as something other than a regular
/// file.
fn key_store_is_present(path: &Path) -> Result<bool, AmError> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the key store at {} exists but is not a regular file, so its bytes cannot be \
                   carried in a snapshot; move it aside or restore the key store, then snapshot again",
                path.display()
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(crate::migrate::map_db_err(
            "reading the key store path",
            error,
        )),
    }
}

/// Refuse a key-store path that does not hold a SQLite key-store database.
///
/// The `.json` in the resolved key-store name is historical: the file is a
/// SQLite database. Copying anything else verbatim would put a foreign
/// document where a key-store database belongs, so this refuses it and leaves
/// the file as it is. A snapshot is a read-only operation and never rewrites
/// the key store to get there.
fn refuse_non_sqlite_store(path: &Path) -> Result<(), AmError> {
    let mut magic = [0u8; MAGIC_LEN];
    let read = match read_magic(path, &mut magic) {
        Ok(read) => read,
        Err(error) => {
            return Err(crate::migrate::map_db_err(
                "reading the key store header",
                error,
            ));
        }
    };
    if read < MAGIC_LEN || magic != SQLITE_MAGIC {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the key store at {} is not a SQLite key-store database; move it aside or restore \
                 the key store, then snapshot again",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Read the leading bytes of `path` into `magic`, returning how many arrived.
///
/// One helper for the open and the read, so the caller maps a single io failure
/// instead of two nearly identical ones.
fn read_magic(path: &Path, magic: &mut [u8]) -> std::io::Result<usize> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    file.read(magic)
}

#[cfg(test)]
#[path = "snapshot_keys_tests.rs"]
mod tests;
