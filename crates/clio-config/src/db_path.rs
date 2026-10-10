// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving one database target from the CLI
//! flag and process-environment precedence chain.
//!
//! ## Owns
//! - The `--db` / `CLIO_DATABASE_URL` / `CLIO_DATA_DIR` / `XDG_DATA_HOME` /
//!   `HOME` precedence order
//! - Backend inference from the database URL scheme
//! - The default SQLite data directory and file name
//!
//! ## Does not own
//! - Creating the database file or its parent directory (store open does that)
//! - Config overlay merge order (see `config::merge`)
//! - Config path/value validation (see `config::validate`)
//!
//! ## Boundary
//!
//! Changes to the database precedence chain or the default data location should
//! normally require changes here. Changes to store opening or config merging
//! should not require changes here.
//!
//! Keep this module focused on pure database-target resolution.

use crate::db_url_env::database_url_from;
use std::path::PathBuf;

/// Base file name written under the resolved data directory.
pub const DB_FILE_NAME: &str = "clio.db";

/// Application subdirectory under the XDG data home.
pub const APP_DIR_NAME: &str = "clio";

/// One resolved database target: the value to open plus the backend to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDatabase {
    /// Database URL or filesystem path handed to the backend opener.
    pub url: String,
    /// Explicit backend when given, else the scheme-inferred `sqlite`/`postgres`.
    pub backend: String,
}

/// Infer the backend from a database URL scheme (config consistency).
///
/// `postgres://` and `postgresql://` are Postgres; everything else — including
/// filesystem paths and `sqlite::memory:` — is SQLite.
#[must_use]
pub fn infer_backend(url: &str) -> &'static str {
    let trimmed = url.trim();
    if trimmed.starts_with("postgres://") || trimmed.starts_with("postgresql://") {
        "postgres"
    } else {
        "sqlite"
    }
}

/// The SQLite file a `sqlite:` / `sqlite://` address names, with the scheme
/// removed.
///
/// A scheme prefix on a SQLite address is a spelling, not part of the file: the
/// key-store path and the backup slug already strip it, so an address that kept
/// it would put the data somewhere the other two resolvers do not look. It is
/// stripped here, at the point every consumer reads, so `sqlite:///data/clio.db`
/// and `/data/clio.db` name one file.
///
/// `sqlite::memory:` is returned untouched. It is not a file, it is the sentinel
/// every open path compares against to choose the in-memory constructor, and
/// stripping the scheme would leave `:memory:` and break that comparison.
///
/// Pure: a function of its argument only. Returns a borrowed slice of the input,
/// so no allocation and no path is ever rewritten.
#[must_use]
pub fn sqlite_file_path(url: &str) -> &str {
    if url == MEMORY_DB {
        return url;
    }
    url.strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url)
}

/// The in-memory SQLite sentinel, which is an address rather than a file path.
const MEMORY_DB: &str = "sqlite::memory:";

/// Default SQLite file path from `CLIO_DATA_DIR`, `XDG_DATA_HOME`, or `HOME`.
///
/// Falls back to the relative `clio.db` when none of those is usable.
/// Pure: no filesystem access. Path joining normalizes trailing separators.
#[must_use]
pub fn default_sqlite_path<F>(get: &F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(dir) = env_value(get, "CLIO_DATA_DIR") {
        return PathBuf::from(dir).join(DB_FILE_NAME);
    }
    if let Some(dir) = env_value(get, "XDG_DATA_HOME") {
        return PathBuf::from(dir).join(APP_DIR_NAME).join(DB_FILE_NAME);
    }
    if let Some(home) = env_value(get, "HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join(APP_DIR_NAME)
            .join(DB_FILE_NAME);
    }
    PathBuf::from(DB_FILE_NAME)
}

/// Resolve the database target with `--db` > `CLIO_DATABASE_URL` >
/// `CLIO_DATA_DIR` > `XDG_DATA_HOME` > `HOME` > relative.
///
/// The URL tier is read through its owner (`db_url_env`), so this resolver and
/// every other reader of the same variable cannot drift apart.
/// A `sqlite:` / `sqlite://` scheme is stripped from a SQLite address, so the
/// resolved `url` is the file every consumer opens and not a string that still
/// spells the scheme. `sqlite::memory:` is the one address left intact, because
/// it is a sentinel rather than a path. An explicit `--backend` always wins over
/// inference. The lookup is injected so tests never mutate process globals.
#[must_use]
pub fn resolve_database<F>(
    get: &F,
    explicit_db: Option<&str>,
    explicit_backend: Option<&str>,
) -> ResolvedDatabase
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(db) = explicit_db.filter(|value| !value.trim().is_empty()) {
        return ResolvedDatabase {
            backend: backend_or_infer(explicit_backend, db),
            url: sqlite_file_path(db).to_owned(),
        };
    }
    if let Some(url) = database_url_from(get) {
        return ResolvedDatabase {
            backend: backend_or_infer(explicit_backend, &url),
            url: sqlite_file_path(&url).to_owned(),
        };
    }
    ResolvedDatabase {
        backend: explicit_backend.unwrap_or("sqlite").to_owned(),
        url: default_sqlite_path(get).to_string_lossy().into_owned(),
    }
}

fn backend_or_infer(explicit_backend: Option<&str>, url: &str) -> String {
    explicit_backend.map_or_else(|| infer_backend(url).to_owned(), str::to_owned)
}

/// Trimmed, non-empty environment value with no interior NUL.
///
/// Empty/whitespace and NUL-bearing values are treated as absent so the next
/// precedence tier applies instead of constructing an unusable path. The bound
/// is `FnMut`, and the lookup arrives by value rather than behind a shared
/// reference, so a caller holding only a mutable, boxed lookup — the store's
/// shape — reaches it too, while a caller holding a shared reference to a
/// non-mutating lookup passes that reference straight through.
pub(crate) fn env_value<F>(mut get: F, key: &str) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    get(key)
        .map(|raw| raw.trim().to_owned())
        .filter(|value| !value.is_empty() && !value.contains('\0'))
}

#[cfg(test)]
#[path = "db_path_tests.rs"]
mod tests;
