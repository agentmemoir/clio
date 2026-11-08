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
//! - The marker's own fields, compared against the marker the incoming payload
//!   would write, so a version or catalog change alone rewrites the marker
//! - The conflict reasons: a locally edited owned file, a missing owned file, an
//!   unexpected file inside the package directory, a package an earlier clio
//!   installed without a marker
//! - What `--force` turns a conflict into, and which foreign files it keeps
//! - Reading the package directory as a map of relative path to file digest
//! - The name of the installer's own temp sibling, which the scan leaves out
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
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What an installation should do to one package directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallPlan {
    /// The package directory already holds the incoming payload, every recorded
    /// file is unchanged, the marker on disk already records this build's
    /// payload, and nothing else is there.
    NoOp,
    /// These paths must be written, in this order: the assets whose bytes differ,
    /// then the marker when the one on disk differs from the one this build would
    /// write in any field. A list holding only the marker means every asset is
    /// already current and the marker alone is stale.
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
    /// The package directory holds files and no marker at all, because an earlier
    /// clio installed the package before markers existed, so nothing on disk
    /// records which of those files this program put there.
    EarlierInstall { path: String },
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
            | Self::EarlierInstall { path }
            | Self::UnreadableMarker { path } => path,
        }
    }
}

/// Decide what to do with the package directory described by `on_disk`.
///
/// `on_disk` is the package directory alone, as [`scan_package_dir`] returns it,
/// so the parent skill root never takes part in the decision. `expected` is the
/// marker the incoming payload produces, so a marker whose installer, package
/// version, catalog hash, payload digest, or recorded file map differs from this
/// build's asks for a write of the marker and nothing else. Nothing is written
/// here and nothing beyond the arguments is read.
#[must_use]
pub(crate) fn plan_install(
    recorded: Option<&SkillMarker>,
    expected: &SkillMarker,
    on_disk: &BTreeMap<String, String>,
    force: bool,
) -> InstallPlan {
    let mut files = Vec::new();
    for (name, hash) in &expected.files {
        if on_disk.get(name) != Some(hash) {
            files.push(name.clone());
        }
    }
    let preserved_foreign: Vec<String> = on_disk
        .keys()
        .filter(|path| path.as_str() != SKILL_MARKER_FILE)
        .filter(|path| !expected.files.contains_key(path.as_str()))
        .cloned()
        .collect();
    let mut reasons = conflict_reasons(recorded, expected, on_disk);
    if !reasons.is_empty() {
        if !force {
            reasons.sort_by(|left, right| left.path().cmp(right.path()));
            return InstallPlan::Conflict { reasons };
        }
        return InstallPlan::Write {
            files: with_marker(recorded, expected, files),
            preserved_foreign,
        };
    }
    let files = with_marker(recorded, expected, files);
    if files.is_empty() {
        return InstallPlan::NoOp;
    }
    InstallPlan::Write {
        files,
        preserved_foreign,
    }
}

/// Add the marker to `files` when the one on disk differs from `expected` in any
/// field. The comparison is the marker as a whole, so a stale installer version,
/// package version, catalog hash, payload digest, or file map is a write, and a
/// marker that already matches every field stays untouched.
fn with_marker(
    recorded: Option<&SkillMarker>,
    expected: &SkillMarker,
    mut files: Vec<String>,
) -> Vec<String> {
    if recorded != Some(expected) {
        files.push(SKILL_MARKER_FILE.to_owned());
    }
    files
}

/// Every reason `on_disk` conflicts with `recorded`, in discovery order.
fn conflict_reasons(
    recorded: Option<&SkillMarker>,
    expected: &SkillMarker,
    on_disk: &BTreeMap<String, String>,
) -> Vec<ConflictReason> {
    match recorded {
        Some(marker) => recorded_reasons(marker, on_disk),
        None => earlier_install_reasons(expected, on_disk),
    }
}

/// Every reason a package that carries a marker conflicts with it.
fn recorded_reasons(
    marker: &SkillMarker,
    on_disk: &BTreeMap<String, String>,
) -> Vec<ConflictReason> {
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

/// Every reason a package with no marker conflicts with `expected`.
///
/// A markerless package whose files already equal the incoming payload exactly —
/// the same relative paths, the same bytes, and no extra file — is not a
/// conflict: an earlier clio installed it and recorded nothing, so the only work
/// left is the marker, which the caller plans as a marker-only write. A package
/// directory holding no file at all is a first installation, not a conflict.
/// Anything else keeps refusing, because nothing on disk says what this program
/// owns there.
fn earlier_install_reasons(
    expected: &SkillMarker,
    on_disk: &BTreeMap<String, String>,
) -> Vec<ConflictReason> {
    let mut paths: BTreeSet<&str> = on_disk
        .keys()
        .filter(|path| path.as_str() != SKILL_MARKER_FILE)
        .map(String::as_str)
        .collect();
    if paths.is_empty() {
        return Vec::new();
    }
    paths.extend(expected.files.keys().map(String::as_str));
    paths
        .into_iter()
        .filter(|path| on_disk.get(*path) != expected.files.get(*path))
        .map(|path| ConflictReason::EarlierInstall {
            path: path.to_owned(),
        })
        .collect()
}

/// The hidden temp sibling the installer's atomic write creates beside a file: a
/// leading dot, the file's own name, the writing process, the clock, and a
/// counter, then `.tmp`. Named here, once, so the writer that produces it and the
/// scan that must not mistake it for a package file agree on one pattern.
#[must_use]
pub(crate) fn temp_sibling_name(name: &str, pid: u32, nanos: u128, seq: u64) -> String {
    format!(".{name}.{pid}.{nanos}.{seq}.tmp")
}

/// Whether `name` is one of the installer's own temp siblings.
///
/// A crash, a full disk, or two concurrent installs can leave one behind. The
/// user never created it and it belongs to no installation, so it is left out of
/// the plan: it is neither written, nor reported, nor a conflict, and it is never
/// deleted.
#[must_use]
pub(crate) fn is_temp_sibling(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(".tmp"))
    else {
        return false;
    };
    let mut fields = rest.rsplitn(4, '.');
    let (Some(seq), Some(nanos), Some(pid), Some(file)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    !file.is_empty()
        && [pid, nanos, seq]
            .iter()
            .all(|field| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Hash every file the package directory `dir` holds, keyed by its path relative
/// to `dir` with `/` separators, so the result is comparable across platforms.
///
/// A directory that does not exist holds no files: a first installation plans
/// from an empty map. A leftover temp sibling of this program's own atomic write
/// is not a package file and is skipped, so it neither refuses an installation nor
/// appears in a plan.
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
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_temp_sibling)
        {
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
