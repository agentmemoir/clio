// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for where the durable erasure journal lives and
//! for minting the deployment identity the journal carries.
//!
//! ## Owns
//! - The `CLIO_ERASURE_JOURNAL_FILE` override and the `<data-dir>` fallback
//! - Minting a deployment identity: 32 random bytes as lowercase hex
//!
//! ## Does not own
//! - Reading, writing, or validating the journal file, its owner-only
//!   permissions, or its atomic-write sequence (the journal store owns those)
//! - Deployment identity checks, generation, denial lookup, and handoff (the
//!   journal authority owns those)
//!
//! ## Boundary
//!
//! Changes to where the journal file lives, or to how a deployment identity is
//! minted, should normally require changes here. Changes to the journal's
//! content, permissions, or checks should not.
//!
//! Keep this module focused on journal placement and deployment identity.
//!
//! # Why the journal is a sibling file
//!
//! The journal is the authority a restore consults, so it must stay outside
//! everything a supported restore replaces: the SQLite database file and its
//! sidecars, the KMS key store, the Postgres schema and rows, and the backup
//! directory. A file beside the database in the data directory is none of those.

use std::path::{Path, PathBuf};

use clio_types::{AmError, ErrorCode};

use crate::db_path::{default_sqlite_path, env_value};

/// Environment variable naming an explicit erasure-journal file path.
pub const ERASURE_JOURNAL_FILE_ENV: &str = "CLIO_ERASURE_JOURNAL_FILE";

/// Default journal file name under the resolved data directory.
pub const ERASURE_JOURNAL_FILE_NAME: &str = "erasure-journal.json";

/// Length of a deployment identity in bytes, rendered as 64 hex characters.
const DEPLOYMENT_ID_LEN: usize = 32;

/// Resolve the erasure-journal path: `CLIO_ERASURE_JOURNAL_FILE` when set, else
/// `<data-dir>/erasure-journal.json`.
///
/// A blank or whitespace-only override counts as unset, matching every other
/// environment override in this crate. An explicit override is honoured even
/// when relative, because naming a path is the operator's decision; the
/// fallback refuses instead, because a journal that followed the process's
/// working directory would not stay outside the paths a restore replaces.
///
/// The lookup is injected so tests never read the host environment.
///
/// # Errors
/// [`ErrorCode::ConfigCorrupt`] when no override is set and no absolute data
/// directory can be resolved; the message names the override that fixes it.
pub fn resolve_journal_path<F>(get: &F) -> Result<PathBuf, AmError>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = env_value(get, ERASURE_JOURNAL_FILE_ENV) {
        return Ok(PathBuf::from(explicit));
    }
    let data_dir = default_sqlite_path(get)
        .parent()
        .map(Path::to_path_buf)
        .filter(|dir| dir.is_absolute());
    data_dir
        .map(|dir| dir.join(ERASURE_JOURNAL_FILE_NAME))
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "no erasure-journal path could be resolved; set \
                     {ERASURE_JOURNAL_FILE_ENV} to a writable path and run again"
                ),
            )
        })
}

/// Mint a deployment identity: 32 random bytes as 64 lowercase hex characters.
///
/// A journal carries this value as its deployment-wide identity, minted once at
/// new-empty initialization and preserved across restart and handoff. The
/// random source is the same `rand::random` the encryption key uses, so the
/// project keeps one CSPRNG rather than two.
///
/// # Panics
/// If the operating system's random source fails, which `rand::random` reports
/// by panicking rather than by returning.
#[must_use]
pub fn new_deployment_id() -> String {
    use std::fmt::Write as _;
    let mut id = String::with_capacity(DEPLOYMENT_ID_LEN * 2);
    for byte in rand::random::<[u8; DEPLOYMENT_ID_LEN]>() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[cfg(test)]
#[path = "erasure_journal_path_tests.rs"]
mod tests;
