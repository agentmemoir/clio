// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deciding what an installation of the embedded
//! skill package should do to one package directory: nothing, a set of writes, or
//! a refusal naming every reason.
//!
//! ## Owns
//! - The recorded-versus-incoming-versus-on-disk comparison
//! - The conflict reasons: a locally edited owned file, a missing owned file, an
//!   unexpected file inside the package directory, an unrecorded package
//! - What `--force` turns a conflict into, and which foreign files it keeps
//! - Reading the package directory as a map of relative path to file digest
//!
//! ## Does not own
//! - What a marker records, or how its digest is taken (`skill_marker`)
//! - Writing any asset or the marker (`skill_install`)
//! - Which skill roots exist, or how a harness maps to one (`agent_harness`)
//!
//! ## Boundary
//!
//! Changes to when an installation writes, refuses, or does nothing should
//! normally require changes here. Changes to how a file is written should not.
//!
//! Only the package directory itself is scanned. A file beside it, in the skill
//! root that holds other packages, is never ours to judge and never appears in a
//! plan.
//!
//! Keep this module focused on the decision, with no writes and no state.

use crate::agent_harness_write::WriteError;
use crate::skill_marker::{SKILL_MARKER_FILE, SkillMarker, sha256_hex};
use std::collections::BTreeMap;
use std::path::Path;

/// What an installation should do to one package directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallPlan {
    /// The package directory already holds the incoming payload, every recorded
    /// file is unchanged, and nothing else is there.
    NoOp,
    /// These assets must be written. `preserved_foreign` lists the files inside
    /// the package directory that no marker records and no asset replaces: they
    /// are reported and kept, never deleted.
    Write {
        files: Vec<String>,
        preserved_foreign: Vec<String>,
    },
    /// Nothing may be written until a person resolves every reason.
    Conflict { reasons: Vec<ConflictReason> },
}

/// One reason an installation refuses to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictReason {
    /// A recorded file's bytes differ from the hash the marker recorded, so
    /// writing would discard a local edit.
    LocalEdit { path: String },
    /// A file the marker records is gone from the package directory.
    MissingOwned { path: String },
    /// The package directory holds a file no marker recorded.
    UnexpectedFile { path: String },
    /// The package directory holds a file, and no marker says this program put
    /// it there.
    Unrecorded { path: String },
    /// The package directory holds a marker this program cannot read, so no file
    /// it holds can be attributed to this program.
    UnreadableMarker { path: String },
}

impl ConflictReason {
    /// The relative path this reason names.
    #[must_use]
    pub(crate) fn path(&self) -> &str {
        match self {
            Self::LocalEdit { path }
            | Self::MissingOwned { path }
            | Self::UnexpectedFile { path }
            | Self::Unrecorded { path }
            | Self::UnreadableMarker { path } => path,
        }
    }
}

/// Decide what to do with the package directory described by `on_disk`.
///
/// `on_disk` is the package directory alone, as [`scan_package_dir`] returns it,
/// so the parent skill root never takes part in the decision. Nothing is written
/// here and nothing beyond the arguments is read.
#[must_use]
pub(crate) fn plan_install(
    marker: Option<&SkillMarker>,
    incoming: &[(&str, &str)],
    on_disk: &BTreeMap<String, String>,
    force: bool,
) -> InstallPlan {
    let incoming_hashes: BTreeMap<&str, String> = incoming
        .iter()
        .filter(|(name, _)| *name != SKILL_MARKER_FILE)
        .map(|(name, text)| (*name, sha256_hex(text.as_bytes())))
        .collect();
    let files: Vec<String> = incoming
        .iter()
        .filter(|(name, _)| *name != SKILL_MARKER_FILE)
        .map(|(name, _)| *name)
        .filter(|name| on_disk.get(*name) != incoming_hashes.get(*name))
        .map(str::to_owned)
        .collect();
    let preserved_foreign: Vec<String> = on_disk
        .keys()
        .filter(|path| path.as_str() != SKILL_MARKER_FILE)
        .filter(|path| !incoming_hashes.contains_key(path.as_str()))
        .cloned()
        .collect();
    let mut reasons = conflict_reasons(marker, on_disk);
    if !reasons.is_empty() {
        if !force {
            reasons.sort_by(|left, right| left.path().cmp(right.path()));
            return InstallPlan::Conflict { reasons };
        }
        return InstallPlan::Write {
            files,
            preserved_foreign,
        };
    }
    if files.is_empty() {
        return InstallPlan::NoOp;
    }
    InstallPlan::Write {
        files,
        preserved_foreign,
    }
}

/// Every reason `on_disk` conflicts with `marker`, in discovery order.
fn conflict_reasons(
    marker: Option<&SkillMarker>,
    on_disk: &BTreeMap<String, String>,
) -> Vec<ConflictReason> {
    let Some(marker) = marker else {
        return on_disk
            .keys()
            .filter(|path| path.as_str() != SKILL_MARKER_FILE)
            .map(|path| ConflictReason::Unrecorded { path: path.clone() })
            .collect();
    };
    let mut reasons = Vec::new();
    for (path, recorded) in &marker.files {
        match on_disk.get(path) {
            None => reasons.push(ConflictReason::MissingOwned { path: path.clone() }),
            Some(found) if found != recorded => {
                reasons.push(ConflictReason::LocalEdit { path: path.clone() });
            }
            Some(_) => {}
        }
    }
    for path in on_disk.keys() {
        if path.as_str() != SKILL_MARKER_FILE && !marker.files.contains_key(path) {
            reasons.push(ConflictReason::UnexpectedFile { path: path.clone() });
        }
    }
    reasons
}

/// Hash every file the package directory `dir` holds, keyed by its path relative
/// to `dir` with `/` separators, so the result is comparable across platforms.
///
/// A directory that does not exist holds no files: a first installation plans
/// from an empty map.
///
/// # Errors
/// A file or directory under `dir` cannot be read.
pub(crate) fn scan_package_dir(dir: &Path) -> Result<BTreeMap<String, String>, WriteError> {
    let mut found = BTreeMap::new();
    walk(dir, dir, &mut found)?;
    Ok(found)
}

fn walk(root: &Path, dir: &Path, found: &mut BTreeMap<String, String>) -> Result<(), WriteError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error("read", dir, &error)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| io_error("read", dir, &error))?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|error| io_error("inspect", &path, &error))?
            .is_dir()
        {
            walk(root, &path, found)?;
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let bytes = std::fs::read(&path).map_err(|error| io_error("read", &path, &error))?;
        found.insert(
            relative.to_string_lossy().replace('\\', "/"),
            sha256_hex(&bytes),
        );
    }
    Ok(())
}

fn io_error(action: &str, path: &Path, error: &std::io::Error) -> WriteError {
    WriteError::Io {
        detail: format!("failed to {action} {}: {error}", path.display()),
    }
}

#[cfg(test)]
#[path = "skill_install_plan_tests.rs"]
mod tests;
