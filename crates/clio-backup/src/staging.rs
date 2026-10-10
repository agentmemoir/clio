// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the staging discipline a snapshot write
//! depends on: the owner-only temporary files that live beside the archive and
//! the digest taken while a member is staged.
//!
//! ## Owns
//! - `StagedFile`: a measured, owner-only temporary member that removes itself
//! - `stage_copy`: copying a source file into a staged member while digesting it
//! - `StagedGuard`: the path cleanup that makes a failed `stage_copy` leave
//!   nothing on disk
//! - `reserve_path` / `begin_staged` / `adopt_staged`: the name reservation and
//!   the handoff that makes one value own a staged path
//! - `ensure_backup_dir`: creating the backup directory owner-only when missing
//! - `io_error` and `io_code`: the step that failed, and the code the
//!   filesystem's own classification is reported as
//!
//! ## Does not own
//! - Publishing a finished file under its final name (see `atomic_publish`)
//! - Member order or archive bytes (see `container`)
//! - What a member contains (see `create`)
//! - Directory precedence (see `clio-config`'s `backup_path`)
//! - Removing a staged file left behind by a process that was killed rather than
//!   returned from, for the staged members a restore's journal names (see
//!   `staging_journal`, swept at the start of a later restore)
//!
//! ## Boundary
//!
//! Changes to how a member is staged and measured should normally require
//! changes here. Changes to how a finished archive is published should not.
//!
//! Keep this module focused on staging one member.

use crate::container::to_hex;
use clio_types::{AmError, ErrorCode};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Chunk size for streaming a member; fixed so a member larger than memory
/// still stages.
const COPY_CHUNK: usize = 64 * 1024;

/// Distinct suffixes tried when a staged name is already taken.
const NAME_ATTEMPTS: u32 = 8;

/// A staged member file: measured, owner-only, and removed on drop.
///
/// The digest and size are taken while the file is written, so pass two never
/// re-reads a source to measure it. Dropping the value removes the file, so a
/// run that returns leaves no staged payload — or a staged key store, which is
/// key material — on disk outside the archive.
///
/// A signal that kills the process does not return at all and runs no `Drop`,
/// so nothing here claims to clean up after one. Only a restore records its
/// staged members in a journal, so only those are removed by a later restore's
/// sweep. A staged copy nothing journals — `stage_copy`'s copy of the key store
/// during a create, and the payload copy the verify pass stages — is removed by
/// the process that staged it and by nothing else, so a signal leaves it on
/// disk.
#[derive(Debug)]
pub struct StagedFile {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

impl StagedFile {
    /// Take ownership of a file another component already wrote and measured
    /// (the store half writes the snapshot payload itself).
    #[must_use]
    pub fn adopt(path: PathBuf, bytes: u64, sha256: String) -> Self {
        Self {
            path,
            bytes,
            sha256,
        }
    }

    /// Where the staged file lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes staged.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Lowercase-hex SHA-256 recorded while staging.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Open the staged file for the assembly pass.
    ///
    /// # Errors
    /// The staged file cannot be reopened.
    pub fn open(&self) -> Result<std::fs::File, AmError> {
        match std::fs::File::open(&self.path) {
            Ok(file) => Ok(file),
            Err(error) => Err(io_error("reopening a staged member", &error)),
        }
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A staged path that removes itself until ownership is handed on.
///
/// `stage_copy` builds the guard the moment the file exists, so a read, a
/// write, or a flush that fails unlinks the staged file instead of leaving a
/// payload — or a key store, which is key material — beside the archive.
///
/// Ownership moves exactly once. [`StagedGuard::release`] marks the guard and
/// hands the path out, so the returned [`StagedFile`] is the only value left
/// that can remove it, and no path is ever removed twice by two owners.
#[derive(Debug)]
struct StagedGuard {
    path: PathBuf,
    released: bool,
}

impl StagedGuard {
    /// Take responsibility for removing `path`.
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            released: false,
        }
    }

    /// Hand the staged path to the caller, which now owns its removal.
    fn release(mut self) -> PathBuf {
        self.released = true;
        std::mem::take(&mut self.path)
    }
}

impl Drop for StagedGuard {
    fn drop(&mut self) {
        if !self.released {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Create an owner-only staged file for streaming, and hand back its path.
///
/// The caller streams into the returned file and then calls
/// [`adopt_staged`] with the size and digest it observed. If it does not, the
/// file is not adopted and no `StagedFile` exists to remove it, so the caller
/// has to remove the path itself: [`stage_copy`] does that through its guard on
/// every path out of it, and the verify pass does it by hand.
///
/// # Errors
/// The staged file cannot be created.
pub fn begin_staged(dir: &Path, tag: &str) -> Result<(PathBuf, std::fs::File), AmError> {
    let path = reserve_path(dir, tag)?;
    let file = create_owner_only(&path)?;
    Ok((path, file))
}

/// Take ownership of a staged file once its size and digest are known.
#[must_use]
pub fn adopt_staged(path: PathBuf, bytes: u64, sha256: String) -> StagedFile {
    StagedFile::adopt(path, bytes, sha256)
}

/// Reserve a path in `dir` for a staged member, without creating the file.///
/// The caller writes the file itself (the store half produces the payload), so
/// this only has to hand out a name nothing else is using. The path is not
/// created, which is also what lets `VACUUM INTO` accept it.
///
/// # Errors
/// Every attempt name is already taken, or the directory is unusable.
pub fn reserve_path(dir: &Path, tag: &str) -> Result<PathBuf, AmError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let stem = format!("clio-snapshot-{tag}-{}-{nanos}", std::process::id());
    for attempt in 0..NAME_ATTEMPTS {
        let path = dir.join(format!("{stem}-{attempt}.tmp"));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(AmError::new(
        ErrorCode::Internal,
        format!(
            "could not reserve a staging name for the {tag} member in {}",
            dir.display()
        ),
    ))
}

/// Stage `bytes` as a member in `dir`, owner-only, digested as it is written.
///
/// This is how a member the caller already holds as bytes (the Postgres
/// dump's `dump/meta.json`) becomes a measured staged member, the same way a
/// copied file does. The staged file is removed on every failure path.
///
/// # Errors
/// The staged file cannot be created, written, or flushed.
pub fn stage_bytes(bytes: &[u8], dir: &Path, tag: &str) -> Result<StagedFile, AmError> {
    let path = reserve_path(dir, tag)?;
    let mut output = create_owner_only(&path)?;
    let guard = StagedGuard::new(path);
    let result = copy_and_measure(
        &mut std::io::Cursor::new(bytes),
        &mut output,
        "staging member bytes",
    );
    drop(output);
    match result {
        Ok((written, sha256)) => Ok(StagedFile::adopt(guard.release(), written, sha256)),
        Err(error) => Err(error),
    }
}

/// Copy `source` into a staged member in `dir`, digesting it as it is written.
///
/// The bytes are copied as they stand: this is how a key store travels with a
/// snapshot, so nothing here parses or rewrites what it copies. The key store
/// this copies holds per-subject keys already wrapped under the Clio encryption
/// key, and the Clio key is not in it, so what travels is ciphertext.
///
/// The staged file is removed on every failure path — the source cannot be
/// read, the copy cannot be written, or the copy cannot be flushed — and
/// survives only on success, where it becomes the returned [`StagedFile`]. That
/// is why the staged path is wrapped in a [`StagedGuard`] as soon as the file
/// exists: no error return out of this function can leave a staged file behind.
/// A signal that kills the process is not an error return, runs no `Drop`, and
/// leaves the file where it is. Nothing journals the copy this function makes
/// for a create, so a later restore's sweep does not reach it: the copy is
/// removed by the process that staged it, or by nobody at all.
///
/// # Errors
/// The source cannot be read, or the staged copy cannot be written or flushed.
pub fn stage_copy(source: &Path, dir: &Path, tag: &str) -> Result<StagedFile, AmError> {
    let path = reserve_path(dir, tag)?;
    let mut input = match std::fs::File::open(source) {
        Ok(file) => file,
        Err(error) => {
            return Err(io_error(
                &format!("opening {} to stage it", source.display()),
                &error,
            ));
        }
    };
    let mut output = create_owner_only(&path)?;
    let guard = StagedGuard::new(path);
    let result = copy_and_measure(
        &mut input,
        &mut output,
        &format!("staging {}", source.display()),
    );
    // Close the handle before the guard unlinks, so the removal also works on
    // a host that refuses to unlink a file that is still open. On the error arm
    // the guard is still live, so it removes the path as this function returns.
    drop(output);
    match result {
        Ok((bytes, sha256)) => Ok(StagedFile::adopt(guard.release(), bytes, sha256)),
        Err(error) => Err(error),
    }
}

/// Stream `input` into `output`, digesting and counting the bytes, then flush
/// the result and report what was written.
///
/// Split out of [`stage_copy`] so the streaming loop can be exercised against a
/// sink whose write or flush fails part-way, which a real file cannot be made
/// to do from a test. The caller owns the cleanup either way.
///
/// # Errors
/// A read, a write, or the final flush fails; the message names the step that
/// failed.
fn copy_and_measure(
    input: &mut impl Read,
    output: &mut impl StagedSink,
    read_context: &str,
) -> Result<(u64, String), AmError> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; COPY_CHUNK];
    let mut written: u64 = 0;
    loop {
        let read = input
            .read(&mut buf)
            .map_err(|error| io_error(read_context, &error))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buf[..read])
            .map_err(|error| io_error("writing a staged member", &error))?;
        hasher.update(&buf[..read]);
        written += read as u64;
    }
    output
        .sync()
        .map_err(|error| io_error("flushing a staged member", &error))?;
    Ok((written, to_hex(&hasher.finalize())))
}

/// What the staging loop needs from a staged member: the `Write` calls, and the
/// flush to disk that makes the measured size and digest durable before the
/// member is used.
///
/// This is a private trait rather than a bare file so the loop can be exercised
/// against a sink whose write or flush fails part-way, which a real file cannot
/// be made to do from a test.
trait StagedSink: Write {
    /// Flush the staged bytes to disk.
    fn sync(&mut self) -> std::io::Result<()>;
}

impl StagedSink for std::fs::File {
    fn sync(&mut self) -> std::io::Result<()> {
        self.sync_all()
    }
}

/// Create the backup directory when missing, owner-only.
///
/// An existing directory is left exactly as the operator left it, so a
/// deliberately widened directory is never silently tightened.
///
/// # Errors
/// The directory cannot be created.
pub fn ensure_backup_dir(path: &Path) -> Result<bool, AmError> {
    if path.is_dir() {
        return Ok(false);
    }
    std::fs::create_dir_all(path).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("cannot create backup directory `{}`: {e}", path.display()),
        )
    })?;
    clio_config::config::restrict_dir(path);
    Ok(true)
}

/// The code a failed io step is reported as.
///
/// The filesystem's own classification is kept where the standard library gives
/// one: a volume that is full or over its quota is the machine's condition, not
/// a fault in this code, and the two need different answers from the operator, so
/// `StorageFull` and `QuotaExceeded` become [`ErrorCode::QuotaExceeded`]. Every
/// other kind stays [`ErrorCode::Internal`], and a permission failure in
/// particular is not called a quota: it is a different problem with a different
/// fix, and mislabelling it would send the operator to the wrong place.
fn io_code(error: &std::io::Error) -> ErrorCode {
    match error.kind() {
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => {
            ErrorCode::QuotaExceeded
        }
        _ => ErrorCode::Internal,
    }
}

/// Wrap an io error with the step that failed, keeping the code [`io_code`] gives
/// it.
pub(crate) fn io_error(context: &str, err: &std::io::Error) -> AmError {
    AmError::new(io_code(err), format!("{context}: {err}"))
}

/// Create a fresh file that only its owner can read.
///
/// The archive carries key material and the store's ciphertext, so a staged or
/// half-written file is owner-only from the first byte, on every path. The
/// publish half calls this for the archive's own temporary file.
pub(crate) fn create_owner_only(path: &Path) -> Result<std::fs::File, AmError> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(error) => Err(io_error(&format!("creating {}", path.display()), &error)),
    }
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;
