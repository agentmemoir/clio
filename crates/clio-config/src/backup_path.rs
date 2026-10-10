// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the backup directory and the
//! store-slug file-name component beside `db_path` and `kms_path`.
//!
//! ## Owns
//! - The `CLIO_BACKUP_DIR` / data-directory / relative-fallback precedence
//!   chain, with an always-absolute result
//! - `store_slug`: a filesystem-safe, credential-free short id for a database
//!
//! ## Does not own
//! - Creating the directory or its permissions (the CLI does, with
//!   `restrict_dir`)
//! - Database-target resolution (see `db_path`) or key-store resolution (see
//!   `kms_path`)
//! - Archive format details (see `clio-backup`)
//!
//! ## Boundary
//!
//! Changes to the backup-directory precedence chain or slug safety rules
//! should normally require changes here. Changes to database or key-store
//! resolution should not require changes here.
//!
//! Keep this module focused on pure backup-directory and slug resolution.

use crate::db_path::{default_sqlite_path, env_value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Environment variable naming an explicit backup directory.
pub const BACKUP_DIR_ENV: &str = "CLIO_BACKUP_DIR";

/// Directory name under the resolved data directory.
pub const BACKUPS_DIR_NAME: &str = "backups";

/// Last-resort directory name, resolved against the current directory.
pub const FALLBACK_DIR_NAME: &str = "clio-backups";

/// Maximum slug length in bytes (allowed characters are all ASCII, so bytes
/// equal characters).
pub const MAX_SLUG_BYTES: usize = 64;

/// Which precedence tier supplied a resolved backup directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupDirTier {
    /// `CLIO_BACKUP_DIR` named the directory.
    EnvOverride,
    /// The data directory's `backups` sibling (`db_path`'s default chain).
    DataDirDefault,
    /// No data directory configured: `clio-backups` beside the current
    /// directory.
    RelativeFallback,
}

impl BackupDirTier {
    /// Human name printed by `clio backup dir`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvOverride => "CLIO_BACKUP_DIR override",
            Self::DataDirDefault => "data directory default",
            Self::RelativeFallback => "current-directory fallback",
        }
    }
}

/// One resolved backup directory: where snapshots go and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBackupDir {
    /// Always-absolute directory path.
    pub path: PathBuf,
    /// The precedence tier that supplied `path`.
    pub tier: BackupDirTier,
}

/// The non-env backup directory: the data directory's `backups` sibling, else
/// the relative fallback resolved against `current_dir`.
///
/// Pure: no filesystem access. `current_dir` is injected so the always-absolute
/// contract is testable without touching process state.
#[must_use]
pub fn default_backup_dir<F>(get: &F, current_dir: &Path) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    resolve_backup_dir(get, current_dir).path
}

/// Resolve the backup directory with the precedence `CLIO_BACKUP_DIR` >
/// the data directory's `backups` sibling > relative `clio-backups`.
///
/// The result is always absolute: a relative value (the last-resort tier, or a
/// relative `CLIO_BACKUP_DIR`) is resolved against the injected current
/// directory before it is returned. The last-resort tier applies when the data
/// directory itself is unset (`default_sqlite_path` falls back to the bare
/// relative `clio.db`, which has no real parent). The lookup is injected so
/// tests never mutate process globals.
#[must_use]
pub fn resolve_backup_dir<F>(get: &F, current_dir: &Path) -> ResolvedBackupDir
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = env_value(get, BACKUP_DIR_ENV) {
        return ResolvedBackupDir {
            path: make_absolute(PathBuf::from(explicit), current_dir),
            tier: BackupDirTier::EnvOverride,
        };
    }
    // `clio.db` with no data-directory tier has an empty parent; that is the
    // no-data-directory case, not a backup directory beside the process root.
    let default_db = default_sqlite_path(get);
    let data_parent = default_db
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    match data_parent {
        Some(parent) => ResolvedBackupDir {
            path: make_absolute(parent.join(BACKUPS_DIR_NAME), current_dir),
            tier: BackupDirTier::DataDirDefault,
        },
        None => ResolvedBackupDir {
            path: make_absolute(PathBuf::from(FALLBACK_DIR_NAME), current_dir),
            tier: BackupDirTier::RelativeFallback,
        },
    }
}

/// A filesystem-safe short id for a database target: the SQLite file stem, or
/// the Postgres database name taken from the URL path.
///
/// The slug never contains a path separator, a URL scheme or credential, or
/// whitespace: every character outside ASCII letters, digits, `.`, `-`, `_` is
/// replaced with `_`, and the result is bounded to [`MAX_SLUG_BYTES`]. A slug
/// that would reduce to nothing (or to `.`/`..`) falls back to a short hash of
/// the resolved target. The database name is taken from the URL path only, so
/// a password in the URL never reaches the slug.
#[must_use]
pub fn store_slug(database_url: &str, backend: &str) -> String {
    let raw = if backend == "postgres" {
        postgres_database_name(database_url)
    } else {
        sqlite_file_stem(database_url)
    };
    let sanitized = sanitize_slug(&raw);
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        hash_fallback(database_url)
    } else {
        sanitized
    }
}

/// Absolute-ize a resolved path against the injected current directory.
fn make_absolute(path: PathBuf, current_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        current_dir.join(path)
    }
}

/// The SQLite slug source: the file stem of the database path.
///
/// `sqlite:` / `sqlite://` prefixes are stripped so the stem is the file name,
/// never the scheme.
fn sqlite_file_stem(database_url: &str) -> String {
    let path = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .unwrap_or(database_url);
    Path::new(path)
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned())
}

/// Split a URL at its scheme separator, without a regex.
fn split_scheme(url: &str) -> (&str, &str) {
    url.find("://")
        .map_or(("", url), |at| (&url[..at], &url[at + 3..]))
}

/// The Postgres slug source: the database name from the URL path.
///
/// Accepted shapes, after the query and fragment are stripped and the
/// `scheme://` prefix is removed: an authority on its own (no path) yields an
/// empty name; an authority followed by a path yields the last segment of that
/// path. Both read the path only, so a user name, password, host or port in the
/// authority never reaches the slug — an empty name falls back to the short
/// hash.
///
/// A raw `/` inside the userinfo (`postgres://user:pa/ss@host/db`) is not
/// valid URL syntax and makes the authority boundary ambiguous: a `@`-split
/// reads a leaked password as the host, and a first-`/`-split mistakes the
/// userinfo slash for the path. Neither can be ruled out, so the name is
/// treated as unknown — it is dropped, never guessed — and the hash fallback
/// carries the slug. A percent-encoded password (`pa%2Fss`) has no slash at
/// all and yields the path name as usual.
fn postgres_database_name(database_url: &str) -> String {
    let without_query = database_url.split(['?', '#']).next().unwrap_or("");
    let (_, after_scheme) = split_scheme(without_query);
    // The authority runs to the first `/` after the scheme, so the name is
    // whatever follows it. A `/` in the userinfo hides that boundary: nothing
    // is guessed, and the empty name falls back to the hash.
    let Some((authority, path)) = after_scheme.split_once('/') else {
        return String::new();
    };
    if after_scheme
        .rsplit_once('@')
        .is_some_and(|(userinfo, _)| userinfo.contains('/'))
        || authority
            .rsplit_once('@')
            .is_some_and(|(userinfo, _)| userinfo.contains(['/', '?', '#']))
    {
        return String::new();
    }
    path.rsplit('/').next().unwrap_or("").to_owned()
}

/// Keep only slug-safe ASCII, bound the length, and drop nothing else.
fn sanitize_slug(raw: &str) -> String {
    raw.chars()
        .take(MAX_SLUG_BYTES)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The degenerate-target fallback: `db-` plus the first 8 hex digits of the
/// SHA-256 of the resolved target.
fn hash_fallback(database_url: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(database_url.as_bytes());
    let mut out = String::with_capacity(2 * 4 + 3);
    let _ = write!(out, "db-");
    for byte in &digest[..4] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
#[path = "backup_path_tests.rs"]
mod tests;
