// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the key file: where it is, how it is created,
//! and whether the running process may read the operating system's store at all.
//!
//! ## Owns
//! - The key-file path resolution
//! - Creating an owner-only key file
//! - Whether this process may touch the OS secret store, and why
//!
//! ## Does not own
//! - Which store the operator chose (see `secret_key_source`)
//! - The OS secret store itself (see `secret_key_store`)
//! - Generating, validating, and wrapping key bytes (see `secret_key`)
//!
//! ## Boundary
//!
//! Changes to where a key file lives, to how it is created, or to when the
//! keystore may be consulted should normally require changes here. Changes to
//! which store is chosen, or to the key bytes, should not.
//!
//! Keep this module focused on the file store and the gate in front of the other.
//!
//! # Why the keystore is gated rather than merely unpreferred
//!
//! Reading the OS secret store is not free on every platform. On macOS a Keychain
//! read can be answered with a password prompt, so a process that will not use the
//! keystore must not ask it at all rather than ask and discard the answer. That is
//! why a recorded file choice bypasses the store, and does not merely deprioritise
//! it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clio_types::{AmError, ErrorCode};

use crate::secret_key::{KEY_FILE_ENV, KEY_FILE_NAME, KEY_FILE_ONLY_ENV, KEY_LEN};
use crate::secret_key_source::{self, KeySource};

/// Resolve the key-file path: `CLIO_KEY_FILE` when set, else `<data dir>/clio.key`.
///
/// A blank or whitespace-only `CLIO_KEY_FILE` counts as unset, matching every
/// other environment override in this crate. The lookup is injected so tests
/// never read the host environment. `None` means no key file is available.
///
/// A data directory that does not resolve to an absolute path yields `None`. The
/// database resolver falls back to a bare `clio.db` when it has no home
/// directory to work from, and a key file beside that would land in whatever
/// directory the process happened to start in — a source tree, a shared folder,
/// or a container layer. A key that follows the process's working directory is
/// not a key store, so the key file is refused instead. An explicit
/// `CLIO_KEY_FILE` is honoured even when relative, because naming a path is the
/// operator's decision rather than a fallback.
#[must_use]
pub fn key_file_path<F>(get: &F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = env_value(get, KEY_FILE_ENV) {
        return Some(PathBuf::from(explicit));
    }
    let data_dir = crate::db_path::default_sqlite_path(get)
        .parent()?
        .to_path_buf();
    data_dir.is_absolute().then(|| data_dir.join(KEY_FILE_NAME))
}

/// Whether this process may read the operating system's secret store.
///
/// A recorded file choice says no, and so does `CLIO_KEY_FILE_ONLY` when a
/// key-file path resolves. Without a path there is nowhere to keep the key, so
/// dropping the only store that could hold it would turn the setting into data
/// loss, and the bypass is refused.
///
/// The lookup is injected so a test can pin both halves of that rule without
/// mutating the process environment, which every parallel test shares.
#[must_use]
pub fn keystore_is_bypassed<F>(get: &F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    let file = key_file_path(get);
    if env_value(get, KEY_FILE_ONLY_ENV).is_some() && file.is_some() {
        return true;
    }
    secret_key_source::read(secret_key_source::marker_path(file.as_deref()).as_deref())
        .ok()
        .flatten()
        .is_some_and(|source| source == KeySource::File)
}

/// Create a key file with owner-only permissions held from the first byte.
///
/// `create_new` plus the mode in the same `open` call is the shape the generated
/// `.env` already uses: no window exists in which another user can read the key,
/// and a file that appeared since the existence check is never clobbered. The
/// write is flushed before returning, so a key that has been reported as created
/// is on disk.
///
/// # Errors
/// The directory could not be created, or the file could not be opened or written.
pub fn write_key_file(path: &Path, key: &[u8; KEY_LEN]) -> Result<(), AmError> {
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|err| io_error(path, "create the directory for", &err))?;
    }
    let mut file = open_new_owner_only(path).map_err(|err| io_error(path, "create", &err))?;
    file.write_all(key)
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error(path, "write", &err))
}

/// An owner-only file that could not be created or written.
///
/// # Panics
/// Never. The path is rendered, never interpreted.
pub(crate) fn io_error(path: &Path, action: &str, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot {action} the Clio encryption key file {}: {err}. Clio has not stored a new \
             key; fix the path or its permissions and run again",
            path.display()
        ),
    )
}

#[cfg(unix)]
fn open_new_owner_only(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_new_owner_only(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// A trimmed, non-empty environment value.
fn env_value<F>(get: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    get(key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
