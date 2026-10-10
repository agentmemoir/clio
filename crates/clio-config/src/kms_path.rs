// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the local dev-KMS key-store path
//! for one database target.
//!
//! ## Owns
//! - The `CLIO_KMS_FILE` override and the SQLite-sibling / Postgres data-dir
//!   fallback shapes
//! - Treating `sqlite::memory:` as "no persistence" (process-local keys)
//!
//! ## Does not own
//! - Reading or writing the key store (see `clio-store`'s `local_kms_db`)
//! - Database-target resolution (see `db_path`)
//!
//! ## Boundary
//!
//! Changes to where the local dev KMS persists keys should normally require
//! changes here. Changes to database resolution or key encoding should not.
//!
//! Keep this module focused on local dev-KMS path resolution.

use crate::db_path::{default_sqlite_path, env_value, infer_backend};
use std::path::{Path, PathBuf};

/// Default file name for the local dev-KMS key store.
///
/// The name is historical: the store holds a SQLite database (a non-SQLite
/// file at this name is refused, not migrated).
pub const KMS_FILE_NAME: &str = "clio.kms.json";

/// Environment variable naming an explicit local dev-KMS key-store path.
pub const KMS_FILE_ENV: &str = "CLIO_KMS_FILE";

/// Suffix appended to a SQLite database path to name its sibling key store.
pub const KMS_SIBLING_SUFFIX: &str = ".kms.json";

/// Resolve the local dev-KMS key-store path for a database target.
///
/// `None` means the target is `sqlite::memory:` (or empty): the database itself
/// dies with the process, so process-local keys are already correct and no file
/// is written. Otherwise the precedence is: `CLIO_KMS_FILE` > SQLite sibling
/// (`<db>.kms.json`) > Postgres data-dir file (`<data-dir>/clio.kms.json`).
///
/// Postgres has no per-database filesystem target, so one key store per host is
/// the local-dev default; a real deployment points `CLIO_KMS_FILE` at its
/// key-management mount. The lookup is injected so tests never read the host
/// environment.
#[must_use]
pub fn resolve_kms_path<F>(get: &F, database_url: &str) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    let url = database_url.trim();
    if url.is_empty() || url == "sqlite::memory:" {
        return None;
    }
    if let Some(explicit) = env_value(get, KMS_FILE_ENV) {
        return Some(PathBuf::from(explicit));
    }
    if infer_backend(url) == "sqlite" {
        return Some(PathBuf::from(format!("{url}{KMS_SIBLING_SUFFIX}")));
    }
    let data_dir = default_sqlite_path(get)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    Some(data_dir.join(KMS_FILE_NAME))
}

#[cfg(test)]
#[path = "kms_path_tests.rs"]
mod tests;
