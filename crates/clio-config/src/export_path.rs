// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the directory that owns exported
//! bundles.
//!
//! ## Owns
//! - The `CLIO_EXPORT_ROOT` / data-directory / relative-fallback precedence
//!   chain, with an always-absolute result
//! - The tier name a caller reports for the resolved root
//!
//! ## Does not own
//! - Creating the export root or setting its permissions (the caller does)
//! - Confining a caller-supplied destination inside the root (the delivery
//!   seam does)
//! - Database-target resolution (see `db_path`) or the backup directory (see
//!   `backup_path`)
//!
//! ## Boundary
//!
//! Changes to the export-root precedence chain should normally require changes
//! here. Changes to bundle contents, or to how a destination is checked
//! against the root, should not require changes here.
//!
//! Keep this module focused on pure export-root resolution.

use crate::db_path::{default_sqlite_path, env_value};
use std::path::{Path, PathBuf};

/// Environment variable naming an explicit export root.
pub const EXPORT_ROOT_ENV: &str = "CLIO_EXPORT_ROOT";

/// Directory name under the resolved data directory.
pub const EXPORT_DIR_NAME: &str = "exports";

/// Last-resort directory name, resolved against the current directory.
pub const FALLBACK_DIR_NAME: &str = "clio-exports";

/// Which precedence tier supplied a resolved export root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRootTier {
    /// `CLIO_EXPORT_ROOT` named the directory.
    EnvOverride,
    /// The data directory's `exports` sibling (`db_path`'s default chain).
    DataDirDefault,
    /// No data directory configured: `clio-exports` beside the current
    /// directory.
    RelativeFallback,
}

impl ExportRootTier {
    /// Stable name a caller prints or logs for the resolved root.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvOverride => "environment override",
            Self::DataDirDefault => "data directory default",
            Self::RelativeFallback => "relative fallback",
        }
    }
}

/// One resolved export root: where bundles are delivered and why there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExportRoot {
    /// Directory path, absolute whenever a current directory was supplied.
    pub path: PathBuf,
    /// The precedence tier that supplied `path`.
    pub tier: ExportRootTier,
}

/// The export root as the environment alone describes it, without an injected
/// current directory.
///
/// With no environment configured there is no directory to resolve the
/// fallback against, so that tier comes back as the bare [`FALLBACK_DIR_NAME`]
/// component. Callers that need the absolute path call [`resolve_export_root`].
/// Pure: no filesystem access.
#[must_use]
pub fn default_export_root<F>(get: &F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = env_value(get, EXPORT_ROOT_ENV) {
        return PathBuf::from(explicit);
    }
    data_dir_exports(get).unwrap_or_else(|| PathBuf::from(FALLBACK_DIR_NAME))
}

/// Resolve the export root with the precedence `CLIO_EXPORT_ROOT` > the data
/// directory's `exports` sibling > relative `clio-exports`.
///
/// The result is always absolute: a relative value (the last-resort tier, or a
/// relative `CLIO_EXPORT_ROOT`) is resolved against the injected current
/// directory before it is returned. An empty, whitespace-only, or NUL-bearing
/// `CLIO_EXPORT_ROOT` is treated as absent. The lookup and the current
/// directory are injected so tests never touch process state.
#[must_use]
pub fn resolve_export_root<F>(get: &F, current_dir: &Path) -> ResolvedExportRoot
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = env_value(get, EXPORT_ROOT_ENV) {
        return ResolvedExportRoot {
            path: make_absolute(PathBuf::from(explicit), current_dir),
            tier: ExportRootTier::EnvOverride,
        };
    }
    match data_dir_exports(get) {
        Some(path) => ResolvedExportRoot {
            path: make_absolute(path, current_dir),
            tier: ExportRootTier::DataDirDefault,
        },
        None => ResolvedExportRoot {
            path: make_absolute(PathBuf::from(FALLBACK_DIR_NAME), current_dir),
            tier: ExportRootTier::RelativeFallback,
        },
    }
}

/// The data-directory tier: the SQLite file's parent plus `exports`.
///
/// `clio.db` with no data-directory tier has an empty parent, which is the
/// no-data-directory case rather than an export root beside the process root.
fn data_dir_exports<F>(get: &F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    default_sqlite_path(get)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(EXPORT_DIR_NAME))
}

/// Absolute-ize a resolved path against the injected current directory.
fn make_absolute(path: PathBuf, current_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        current_dir.join(path)
    }
}

#[cfg(test)]
#[path = "export_path_tests.rs"]
mod tests;
