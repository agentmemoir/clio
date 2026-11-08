// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deciding which package directories a removal run reads and whether a path on the way to one may be followed at all.
//!
//! ## Owns
//! - Resolving the package directories the selected harness reads, or the portable root when no harness is named
//! - Whether a recorded marker key is a plain relative path inside the package
//! - Whether a recorded file, the marker, the package directory, or any ancestor between the home directory and the package is a symbolic link
//! - The plain IO failure for a path this module inspects
//!
//! ## Does not own
//! - What a marker records, or how its digest is taken (`skill_marker`)
//! - Collecting refusal reasons, deleting files, or deciding an outcome (`skill_uninstall`)
//! - Which root a harness reads, or how a root is joined to the home directory (`agent_harness`, `agent_harness_targets`)
//!
//! ## Boundary
//!
//! Changes to which paths a removal may touch, or to when a path is followed, should normally require changes here. Changes to what a marker records belong in `skill_marker`, and changes to what an outcome prints belong in `skill_uninstall_text`.
//!
//! Keep this module focused on the paths a removal may trust.

use crate::agent_harness::{self, HarnessRecord, SkillRoot};
use crate::agent_harness_targets::{DirectWriteEnv, resolve_skill_root};
use crate::skill_install::SKILL_DIR_NAME;
use crate::skill_marker::SKILL_MARKER_FILE;
use crate::skill_uninstall::UninstallError;
use std::path::{Component, Path, PathBuf};

/// The portable root alone, used when `clio uninstall --skill-only` names no harness.
const PORTABLE_ROOTS: &[SkillRoot] = &[SkillRoot::Portable];

/// Whether `key` is a plain relative path a marker may record: at least one component, every component a normal name, and not the marker file itself.
///
/// An absolute path, a parent step, a current step, a Windows prefix, an empty string, and the marker's own name are all rejected, because joining one to the package directory would leave it or name the marker twice.
pub(crate) fn marker_key_is_relative(key: &str) -> bool {
    if key.is_empty() || key == SKILL_MARKER_FILE {
        return false;
    }
    let path = Path::new(key);
    let mut components = path.components();
    let Some(first) = components.next() else {
        return false;
    };
    matches!(first, Component::Normal(_))
        && components.all(|component| matches!(component, Component::Normal(_)))
}

/// Every package directory, `<root>/clio-mcp`, in mapping order. A root that resolves to a path already in the set is kept once, so a harness whose native root equals the portable root never gets a duplicate.
pub(crate) fn package_dirs(
    record: Option<&HarnessRecord>,
    env: &DirectWriteEnv,
) -> Result<Vec<PathBuf>, UninstallError> {
    let roots = match record {
        Some(record) => agent_harness::skill_roots(record.name),
        None => PORTABLE_ROOTS,
    };
    let missing = match record {
        Some(record) => format!(
            "cannot resolve a skill root for `{}` because the home directory is unknown",
            record.name
        ),
        None => String::from(
            "cannot resolve the portable skill root because the home directory is unknown",
        ),
    };
    let mut packages: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let path = resolve_skill_root(*root, env).ok_or_else(|| UninstallError::Io {
            detail: missing.clone(),
        })?;
        let package = path.join(SKILL_DIR_NAME);
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    Ok(packages)
}

/// Whether any path component from `dir.join(relative)` up to `dir` is a symbolic link. A component that is missing is not a link; the hash comparison reports it as a missing owned file.
pub(crate) fn symlink_component(dir: &Path, relative: &str) -> Result<bool, UninstallError> {
    let path = dir.join(relative);
    let mut current = Some(path.as_path());
    while let Some(candidate) = current {
        let metadata = match std::fs::symlink_metadata(candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io_failure("inspect", candidate, &error)),
        };
        if metadata.file_type().is_symlink() {
            return Ok(true);
        }
        if candidate == dir {
            break;
        }
        current = candidate.parent();
    }
    Ok(false)
}

/// The first symbolic link below `home` on the way to `dir`, including `dir` itself and the skill root, when one is there. The home directory is not checked, so a home that is itself a link is not a refusal.
///
/// A missing component stops the walk: the caller reports a missing package directory as a no-op. Any other inspection failure is returned.
pub(crate) fn ancestor_symlink(home: &Path, dir: &Path) -> Result<Option<PathBuf>, UninstallError> {
    let relative = dir.strip_prefix(home).unwrap_or(dir);
    let mut current = home.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(Some(current)),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_failure("inspect", &current, &error)),
        }
    }
    Ok(None)
}

/// The plain failure for a path this module or the remover could not inspect, read, or remove.
pub(crate) fn io_failure(action: &str, path: &Path, error: &std::io::Error) -> UninstallError {
    UninstallError::Io {
        detail: format!("failed to {action} {}: {error}", path.display()),
    }
}

#[cfg(test)]
#[path = "skill_uninstall_guard_tests.rs"]
mod tests;
