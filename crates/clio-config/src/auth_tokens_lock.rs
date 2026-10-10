// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the exclusive writer hold that guards the
//! token store: the advisory `flock` on `<store>.lock`, its bounded wait, and
//! the refusal a writer that cannot take it receives.
//!
//! ## Owns
//! - `ExclusiveHold` and its drop-time release
//! - The lock file's path, its owner-only creation, and its open mode
//! - The bounded wait and the refusal message naming the store and the lock
//!
//! ## Does not own
//! - What a writer does under the hold (see `auth_tokens::add_token` and
//!   `auth_tokens::revoke_token`)
//! - The record shape, validation, or the store file itself
//! - The item store's own lock and its shared holds (`clio-store/src/store_lock.rs`)
//!
//! ## Boundary
//!
//! Changes to when the token store is locked or refused should normally require
//! changes here. Changes to what a locked writer writes should not.
//!
//! Keep this module focused on the one cross-process writer hold.
//!
//! # Why the store needs one
//!
//! The token store is a whole-file replacement, so two writers that both read
//! before either writes lose one record. On `add` that loss is unrecoverable:
//! the run prints a secret whose row is not on disk, and the operator has an
//! unrecoverable credential no row accounts for. The hold therefore covers the
//! whole read-modify-write, not just the write.
//!
//! The rule follows the item store's (`crates/clio-store/src/store_lock.rs`):
//! one advisory `flock`, a bounded wait, then a refusal that names the lock
//! file. It never waits forever and never lets the second writer silently
//! overwrite the first. The OS releases the hold when the process exits, so a
//! lock file left behind by a dead run never blocks a later one.

use crate::error::{AmError, ErrorCode};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// The suffix appended to the store's file name for its lock file.
const LOCK_SUFFIX: &str = ".lock";

/// How long a writer waits for a contending writer before refusing.
///
/// A whole-file rewrite of a few dozen records takes well under this on a
/// healthy machine, so an ordinary `add` waits out an ordinary `add`, and a
/// writer that cannot get in says so rather than hanging a terminal.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// How often a waiting writer retries its non-blocking attempt.
const LOCK_RETRY: Duration = Duration::from_millis(25);

/// The lock file for one store: the store's own path plus [`LOCK_SUFFIX`],
/// beside it.
#[must_use]
pub fn lock_file_path(store: &Path) -> PathBuf {
    let mut name = store.as_os_str().to_owned();
    name.push(LOCK_SUFFIX);
    PathBuf::from(name)
}

/// An exclusive advisory hold on one store file, released on drop.
#[derive(Debug)]
pub struct ExclusiveHold {
    lock_path: PathBuf,
    file: File,
}

impl ExclusiveHold {
    /// Take the exclusive hold, waiting out a contending writer for
    /// [`LOCK_WAIT`].
    ///
    /// # Errors
    /// [`ErrorCode::Internal`] when the lock file cannot be opened or locked,
    /// [`ErrorCode::InvalidArgument`] when the wait runs out — the same code
    /// the item store uses to refuse an exclusive hold it cannot take.
    pub fn acquire(store: &Path) -> Result<Self, AmError> {
        Self::acquire_with_wait(store, LOCK_WAIT)
    }

    /// The same hold with an explicit wait, so a test can prove the refusal
    /// without waiting the production duration.
    ///
    /// # Errors
    /// As [`Self::acquire`].
    pub fn acquire_with_wait(store: &Path, wait: Duration) -> Result<Self, AmError> {
        let lock_path = lock_file_path(store);
        let file = open_lock_file(&lock_path)?;
        try_bounded(&file, wait, store, &lock_path)?;
        Ok(Self { lock_path, file })
    }

    /// The lock file this hold is on, for a refusal that names it.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for ExclusiveHold {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Retry a non-blocking exclusive lock until it is granted or `wait` runs out.
fn try_bounded(file: &File, wait: Duration, store: &Path, lock_path: &Path) -> Result<(), AmError> {
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(AmError::new(
                    ErrorCode::Internal,
                    format!(
                        "failed to lock the token store lock {}: {error}",
                        lock_path.display()
                    ),
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
        }
        if Instant::now() >= deadline {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "refusing to write the token store at {} while another writer holds it: the \
                     lock {} stayed exclusively held; wait for the other `clio auth token` run to \
                     finish and retry",
                    store.display(),
                    lock_path.display()
                ),
            ));
        }
        std::thread::sleep(LOCK_RETRY);
    }
}

/// Open the lock file, creating it owner-only when missing.
fn open_lock_file(lock_path: &Path) -> Result<File, AmError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)
        .map_err(|error| {
            AmError::new(
                ErrorCode::Internal,
                format!(
                    "failed to open the token store lock {}: {error}",
                    lock_path.display()
                ),
            )
        })
}

#[cfg(test)]
#[path = "auth_tokens_lock_tests.rs"]
mod tests;
