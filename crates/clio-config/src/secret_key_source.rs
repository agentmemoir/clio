// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for which store holds Clio's encryption key, and
//! for recording the operator's one-time choice of it.
//!
//! ## Owns
//! - `KeySource`, the two stores and the name each is recorded under
//! - The `key-source` marker file beside the key, and reading and writing it
//! - Turning "no marker" into a default, so a machine that never chose still works
//!
//! ## Does not own
//! - Generating, reading, or validating key bytes (see `secret_key`)
//! - Asking the operator (see `setup_key_source` in the wizard)
//! - The sealed configuration the key protects
//!
//! ## Boundary
//!
//! Changes to which stores exist, to the marker's name or contents, or to what an
//! absent marker means should normally require changes here. Changes to the key
//! bytes, or to how a store is read, should not.
//!
//! Keep this module focused on the choice of store and the record of it.
//!
//! # Why the marker is not the config
//!
//! The config is sealed with the key, so reading it needs the key, and the key's
//! location is the thing being recorded. Storing the choice in the config would
//! need the key to read the answer to the question that says where the key is.
//!
//! # Why the marker is not the key file's existence
//!
//! Inferring the choice from whether a key file is there looks like one less piece
//! of state, and it is exactly that: a key file that has been deleted or moved
//! reads as "the operator chose the keystore", Clio finds nothing there, generates
//! a new key, and every sealed value becomes unreadable with no error anywhere.
//! A marker that outlives the key turns that into a refusal that names the key.
//!
//! # Why the file store is the default
//!
//! It is the only store that works everywhere: no Secret Service to reach, no
//! login keychain to unlock, nothing that a container or a CI runner lacks. An
//! absent marker therefore means the file store, which is also what the wizard
//! recommends, so a machine that never chose and a machine that chose the
//! recommended option behave the same.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clio_types::{AmError, ErrorCode};

/// The name of the marker file, beside the key file in the data directory.
pub(crate) const MARKER_NAME: &str = "key-source";

/// The name of the sync key's marker, beside the sync key file.
///
/// A second marker rather than a shared map: the data key's marker path is
/// derived from the data key file's directory, and the sync key's is derived
/// from its own file. One marker answering two questions could not keep that
/// derivation, and the two keys must never be able to answer for each other.
pub(crate) const SYNC_MARKER_NAME: &str = "sync-key-source";

/// The two places Clio's encryption key can live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// An owner-only file beside the key file's directory.
    ///
    /// The recommended store: it works on a headless machine, in a container, and
    /// in CI, none of which have an OS secret store to ask.
    File,
    /// The operating system's secret store: Keychain, Credential Manager, or the
    /// Secret Service.
    ///
    /// Stronger than the file, because the operating system protects it with the
    /// same credentials that protect the rest of the user's secrets. It is not the
    /// default because a machine without one cannot use it at all.
    Keystore,
}

impl KeySource {
    /// Every source, in the order the wizard offers them.
    pub const ALL: [Self; 2] = [Self::File, Self::Keystore];

    /// The word the marker records and the flag accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Keystore => "keystore",
        }
    }

    /// The one a machine with no marker uses, and the one the wizard highlights.
    ///
    /// The file store, because it is the only one that works without an OS secret
    /// store. An operator who wants the stronger store picks it, and the marker
    /// records that they did.
    #[must_use]
    pub const fn recommended() -> Self {
        Self::File
    }

    /// Read a source from a flag or a marker, case-insensitively.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        Self::ALL
            .into_iter()
            .find(|source| source.as_str().eq_ignore_ascii_case(raw))
    }

    /// The two names, for an error that lists what was accepted.
    #[must_use]
    pub fn accepted() -> String {
        Self::ALL
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The marker path: beside the key file, in the same directory.
///
/// Derived from the key-file path so the two can never disagree about which
/// directory Clio is configured to use, and so an explicit `CLIO_KEY_FILE` in
/// another directory carries its marker with it.
#[must_use]
pub fn marker_path(key_file: Option<&Path>) -> Option<PathBuf> {
    marker_beside(key_file, MARKER_NAME)
}

/// The sync key's marker path: beside the sync key file, in its directory.
///
/// The same derivation rule as [`marker_path`], for the same reason: the record
/// of the choice and the file it describes can never point at two directories.
#[must_use]
pub fn sync_marker_path(sync_key_file: Option<&Path>) -> Option<PathBuf> {
    marker_beside(sync_key_file, SYNC_MARKER_NAME)
}

/// One marker path under the directory that holds `key_file`.
fn marker_beside(key_file: Option<&Path>, name: &str) -> Option<PathBuf> {
    key_file
        .and_then(Path::parent)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
}

/// The recorded choice, or `None` when no marker exists.
///
/// An unreadable or unrecognised marker is an error rather than a default: it
/// exists, so the operator made a choice, and guessing a different store than the
/// one they chose is the one answer that cannot be walked back.
///
/// # Errors
/// The marker exists but cannot be read, or names a store Clio does not know. Both
/// are refusals rather than a fall back to a default, because the operator did make
/// a choice and this file is the only place it is written down.
pub fn read(marker: Option<&Path>) -> Result<Option<KeySource>, AmError> {
    let Some(path) = marker else {
        return Ok(None);
    };
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "cannot read the Clio key-store record {}: {err}. Clio will not guess which \
                     store you chose, because reading the wrong one cannot be undone; remove the \
                     file to go back to the default",
                    path.display()
                ),
            ));
        }
    };
    let trimmed = raw.trim();
    KeySource::parse(trimmed).map(Some).ok_or_else(|| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "the Clio key-store record {} says `{trimmed}`, which is not a store Clio knows. The \
                 accepted values are {}; Clio has not changed which store it reads",
                path.display(),
                KeySource::accepted()
            ),
        )
    })
}

/// Record the choice, once.
///
/// Written with `create_new`, so a second setup on a machine that already chose
/// cannot overwrite the record. The caller checks [`read`] first, and a refusal
/// here means two runs raced rather than that the operator changed their mind.
///
/// # Errors
/// No key-file directory could be resolved, so the record has nowhere to go, or
/// the filesystem refused to create, open or write it.
pub fn write(marker: Option<&Path>, source: KeySource) -> Result<(), AmError> {
    let path = marker.ok_or_else(|| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            "no key-file directory could be resolved, so the key-store choice has nowhere to be \
             recorded; set CLIO_KEY_FILE to a writable path and run again",
        )
    })?;
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|err| io_error(path, "create the directory for", &err))?;
    }
    let mut file = open_new_owner_only(path).map_err(|err| io_error(path, "create", &err))?;
    file.write_all(source.as_str().as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|err| io_error(path, "write", &err))
}

fn io_error(path: &Path, action: &str, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot {action} the Clio key-store record {}: {err}. Clio has not chosen a key store, \
             so nothing has been written; fix the path or its permissions and run again",
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

/// The recorded choice for this machine, creating nothing.
///
/// `None` means no marker exists, which is the one-time case: the wizard asks.
/// An unreadable marker is `None` here too, because a caller that is only going
/// to report where the key lives has nothing to fix it with; the load path, which
/// has to act on it, reports the error instead.
///
/// The lookup is injected rather than read from the process environment, so a test
/// resolves a marker under its own temporary directory instead of the developer's
/// real data directory. Reading the process environment here would make a test
/// suite write a `key-source` marker into the machine it runs on, and every
/// subsequent run on that machine would then read it and skip the question.
pub fn recorded<F>(get: &F) -> Option<KeySource>
where
    F: Fn(&str) -> Option<String>,
{
    let file = crate::secret_key_file::key_file_path(get);
    read(marker_path(file.as_deref()).as_deref()).ok().flatten()
}

/// Record the choice, or confirm the one already recorded.
///
/// # Errors
/// A marker exists that names a store Clio does not know, or the marker cannot be
/// written. Both leave the choice unmade rather than made wrongly.
///
/// The lookup is injected for the reason [`recorded`] documents: this writes, so a
/// test that reached the process environment would modify the machine it runs on.
pub fn record<F>(get: &F, source: KeySource) -> Result<KeySource, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    let file = crate::secret_key_file::key_file_path(get);
    let marker = marker_path(file.as_deref());
    if let Some(recorded) = read(marker.as_deref())? {
        return Ok(recorded);
    }
    write(marker.as_deref(), source)?;
    Ok(source)
}

#[cfg(test)]
#[path = "secret_key_source_tests.rs"]
mod tests;
