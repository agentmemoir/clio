// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the per-deployment tombstone salt.
//!
//! ## Owns
//! - The `CLIO_COMPLIANCE_SALT` override
//! - First-use generation and owner-only persistence of the deployment salt beside the erasure journal
//! - The owner-only read rule for an existing sidecar
//! - The non-reversible salt fingerprint
//!
//! ## Does not own
//! - Identifier hashing (`clio-store::sha256_hex`)
//! - Erasure-journal placement and locking
//!
//! ## Boundary
//!
//! Changes to how the deployment salt is sourced or stored should normally require changes here.
//! Changes to how identifiers are hashed should not require changes here.
//!
//! Keep this module focused on the deployment salt.
//!
//! The salt is write-once: re-minting it would invalidate every hash recorded
//! under the old one, so an existing sidecar is read and never replaced. The
//! sidecar is read under the owner-only rule the journal applies to itself: a
//! link, another kind of file, or a mode that grants any group or other access
//! is refused, because a readable sidecar lets a local reader confirm a guessed
//! subject exactly as a published salt would.

use clio_store::sha256_hex;
use clio_types::{AmError, ErrorCode};
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::erasure_journal_file::owner_only;
use crate::export_custody::export_custody_files::{fsync_dir, io_error, open_owner_only};

/// The environment override an operator sets to supply the salt directly.
const SALT_ENV: &str = "CLIO_COMPLIANCE_SALT";

/// The exact salt length the persisted sidecar must hold.
const SALT_LEN: usize = 32;

/// The suffix that turns the erasure-journal path into its salt sibling.
const SALT_SUFFIX: &str = ".salt";

/// Domain string keyed by the deployment salt so a fingerprint reveals nothing
/// about the salt itself.
const SALT_FINGERPRINT_DOMAIN: &str = "clio-compliance-salt-fingerprint-v1";

/// The deployment tombstone salt for the journal at `journal_path`.
///
/// A non-blank `CLIO_COMPLIANCE_SALT` wins. Otherwise the salt is read from the
/// owner-only sidecar beside the journal, and generated there on first use.
///
/// # Errors
/// The override is set but blank, the sidecar holds anything but thirty-two
/// bytes, or the sidecar could not be read, created, or flushed.
pub fn tombstone_salt_at(journal_path: &Path) -> Result<Vec<u8>, AmError> {
    resolve_salt(std::env::var(SALT_ENV).ok().as_deref(), journal_path)
}

/// A non-reversible fingerprint of the deployment salt `journal_path` resolves.
///
/// # Errors
/// The same failures [`tombstone_salt_at`] reports.
pub fn tombstone_salt_fingerprint_at(journal_path: &Path) -> Result<String, AmError> {
    let salt = tombstone_salt_at(journal_path)?;
    Ok(tombstone_salt_fingerprint_of(&salt))
}

/// The fingerprint of `salt` under the domain string.
///
/// The archive manifest and the erasure journal each record it, so a restore
/// can refuse an archive whose subject ids were hashed under a different salt
/// than the live authority recorded.
#[must_use]
pub fn tombstone_salt_fingerprint_of(salt: &[u8]) -> String {
    sha256_hex(salt, SALT_FINGERPRINT_DOMAIN)
}

/// Resolve the deployment salt from an explicit override and the journal path.
///
/// The override is passed in rather than read here, so a caller that already
/// holds the value never consults the process environment.
///
/// # Errors
/// The override is set but blank, the sidecar holds anything but thirty-two
/// bytes, or the sidecar could not be read, created, or flushed.
pub(crate) fn resolve_salt(raw: Option<&str>, journal_path: &Path) -> Result<Vec<u8>, AmError> {
    match raw {
        Some(value) => configured_salt(value),
        None => persisted_salt(journal_path),
    }
}

/// An operator-supplied salt. A blank value is a configuration mistake rather
/// than an absent one, so it is refused instead of deferred to the sidecar.
fn configured_salt(value: &str) -> Result<Vec<u8>, AmError> {
    if value.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "{SALT_ENV} is set but holds no salt. A blank value is not an unset value, so \
                 resolution stops here: unset it to use the deployment salt file beside the \
                 erasure journal, or set a non-blank salt"
            ),
        ));
    }
    Ok(value.as_bytes().to_vec())
}

/// The salt persisted beside the journal, read or generated on first use.
fn persisted_salt(journal_path: &Path) -> Result<Vec<u8>, AmError> {
    let path = salt_path(journal_path);
    match read_salt(&path)? {
        Some(salt) => Ok(salt),
        None => create_salt(&path),
    }
}

/// The sidecar salt, or `None` when no sidecar exists.
///
/// An existing sidecar is read under the owner-only rule the erasure journal
/// applies to itself, and it is never regenerated here: replacing it would
/// invalidate every identifier already hashed under the salt it holds.
fn read_salt(path: &Path) -> Result<Option<Vec<u8>>, AmError> {
    // The path is observed without following links, so a link planted at the
    // sidecar name is refused rather than read through.
    let observed = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_error("inspect", path, &err)),
    };
    if let Some(refusal) = untrusted(path, &observed) {
        return Err(refusal);
    }
    let bytes = fs::read(path).map_err(|err| io_error("read", path, &err))?;
    Ok(Some(exact_salt(&bytes, path)?))
}

/// The refusal for a sidecar this build will not read, naming what was found.
///
/// A symbolic link, any non-regular file, and a mode that grants any group or
/// other access are all refused: a salt any local reader can open lets that
/// reader confirm a guessed subject, which is what a per-deployment salt exists
/// to prevent. `None` when the observation is one regular, owner-only file.
fn untrusted(path: &Path, meta: &fs::Metadata) -> Option<AmError> {
    let found = if meta.file_type().is_symlink() {
        "is a symbolic link, and a link is never followed".to_owned()
    } else if !meta.is_file() {
        "is not a regular file".to_owned()
    } else if owner_only(meta) {
        return None;
    } else {
        format!(
            "is mode {}, which grants group or other access",
            mode_bits(meta)
        )
    };
    Some(AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "the deployment salt file {} {found}; it must be one regular file that grants nothing \
             to group or other. The file is not regenerated here, because replacing it would \
             invalidate every identifier already hashed under the salt it holds",
            path.display()
        ),
    ))
}

/// The permission bits of `meta` as four octal digits.
#[cfg(unix)]
fn mode_bits(meta: &fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    format!("{:04o}", meta.permissions().mode() & 0o777)
}

/// Non-Unix platforms have no mode bits, so [`owner_only`] never refuses one and
/// this arm is unreachable.
#[cfg(not(unix))]
fn mode_bits(_meta: &fs::Metadata) -> String {
    "unrestricted".to_owned()
}

/// `bytes` as a salt, or a refusal when they are not exactly [`SALT_LEN`] once
/// surrounding ASCII whitespace is dropped, so a trailing newline is tolerated
/// and a truncated or widened body is not.
///
/// A body already the exact length is taken as written: a generated salt is
/// uniform random bytes, so one of them may itself be a whitespace byte.
fn exact_salt(bytes: &[u8], path: &Path) -> Result<Vec<u8>, AmError> {
    let salt = if bytes.len() == SALT_LEN {
        bytes
    } else {
        bytes.trim_ascii()
    };
    if salt.len() != SALT_LEN {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the deployment salt file {} holds {} bytes; exactly {SALT_LEN} are required. \
                 The file is not regenerated here, because replacing it would invalidate every \
                 identifier already hashed under the old salt",
                path.display(),
                salt.len()
            ),
        ));
    }
    Ok(salt.to_vec())
}

/// Generate the salt and persist it owner-only.
///
/// `create_new` is one `O_CREAT|O_EXCL`, so a planted link or a racing
/// provisioner cannot be written through: the loser of the race re-reads the
/// winner's file under the same length rule.
fn create_salt(path: &Path) -> Result<Vec<u8>, AmError> {
    let dir = salt_dir(path);
    fs::create_dir_all(dir).map_err(|err| io_error("create", dir, &err))?;
    let mut file = match open_owner_only(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            return read_salt(path)?.ok_or_else(|| {
                AmError::new(
                    ErrorCode::Internal,
                    format!(
                        "the deployment salt file {} was provisioned concurrently and cannot be \
                         read back",
                        path.display()
                    ),
                )
            });
        }
        Err(err) => return Err(io_error("create", path, &err)),
    };
    restrict_to_owner(&file, path)?;
    let salt: [u8; SALT_LEN] = rand::random();
    file.write_all(&salt)
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error("write", path, &err))?;
    fsync_dir(dir)?;
    Ok(salt.to_vec())
}

/// The sidecar path of the journal: its own name with the salt suffix pushed
/// on, so a path that is not valid UTF-8 still resolves.
fn salt_path(journal_path: &Path) -> PathBuf {
    let mut name: OsString = journal_path.as_os_str().to_os_string();
    name.push(SALT_SUFFIX);
    PathBuf::from(name)
}

/// The directory the sidecar is created in and flushed with.
fn salt_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Force the salt file to exactly `0600`: the mode the open asked for is masked
/// by the process umask, so it is applied again to the opened file.
#[cfg(unix)]
fn restrict_to_owner(file: &fs::File, path: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|err| io_error("restrict", path, &err))
}

/// Non-Unix platforms have no mode bits to set, so the owner-only rule is a
/// no-op there, exactly as it is for the erasure journal.
#[cfg(not(unix))]
fn restrict_to_owner(_file: &fs::File, _path: &Path) -> Result<(), AmError> {
    Ok(())
}

#[cfg(test)]
#[path = "salt_tests.rs"]
mod salt_tests;
