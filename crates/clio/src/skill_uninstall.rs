// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for removing an installed `clio-mcp` skill package only when its provenance marker proves that clio installed exactly the files on disk, and for retaining a shared copy while another harness still references it.
//!
//! ## Owns
//! - Reading the marker and refusing a package whose installer, payload digest, or file bytes do not match it
//! - Collecting every refusal reason for one directory before anything is deleted
//! - The removal outcome: removed, retained with its remaining harness names, or a no-op
//! - Deleting only the recorded files, pruning their empty parent directories, and removing the package directory only when this installation created it
//!
//! ## Does not own
//! - What the marker records, or how its digest is taken (`skill_marker`)
//! - The install-time plan and its conflict reasons (`skill_install_plan`)
//! - The words these outcomes print (`skill_uninstall_text`)
//! - Which root a harness reads, how a root is joined to the home directory, and whether a path on the way to a package is a symbolic link (`agent_harness`, `agent_harness_targets`, `skill_uninstall_guard`)
//! - Registering or unregistering the MCP server
//!
//! ## Boundary
//!
//! Changes to which files may be deleted, or to when a package is retained, should normally require changes here. Changes to what a marker records belong in `skill_marker`, changes to the printed words belong in `skill_uninstall_text`, and changes to which paths may be trusted belong in `skill_uninstall_guard`.
//!
//! Keep this module focused on removing one recorded skill package.

use crate::agent_harness::HarnessRecord;
use crate::agent_harness_targets::DirectWriteEnv;
use crate::agent_harness_write::WriteError;
use crate::skill_install::write_bytes_atomic;
use crate::skill_install_plan::{ConflictReason, scan_package_dir};
use crate::skill_marker::{
    SKILL_INSTALLER, SKILL_MARKER_FILE, SkillMarker, payload_digest_of_bytes, sha256_hex,
};
use crate::skill_uninstall_guard::{ancestor_symlink, io_failure, package_dirs, symlink_component};
use crate::skill_uninstall_text::{failure_message, outcome_text};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What one removal run did to one package directory.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemovalOutcome {
    /// The package was removed. `files` holds every recorded relative path, and `removed_dir` says whether the package directory itself is gone.
    Removed {
        dir: PathBuf,
        files: Vec<String>,
        removed_dir: bool,
    },
    /// The package stays because another harness still names it. `remaining` is the recorded set without the harness this run named.
    Retained {
        dir: PathBuf,
        remaining: BTreeSet<String>,
    },
    /// No package directory, or an empty package directory with no marker, was there.
    NoOp { dir: PathBuf },
}

/// One fact on disk that does not match the marker, so nothing may be removed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RefusalReason {
    /// A recorded-versus-on-disk conflict the installer already names.
    Conflict(ConflictReason),
    /// The marker records a program other than clio as the installer.
    ForeignInstaller { recorded: String },
    /// The recorded payload digest differs from the digest of the recorded bytes.
    PayloadDigest { recorded: String, found: String },
    /// A path this removal would read or delete is a symbolic link.
    Symlink { path: String },
}

impl RefusalReason {
    /// The path this reason names, in the form the refusal prints.
    pub(crate) fn path(&self) -> &str {
        match self {
            Self::Conflict(reason) => reason.path(),
            Self::ForeignInstaller { .. } | Self::PayloadDigest { .. } => SKILL_MARKER_FILE,
            Self::Symlink { path } => path,
        }
    }
}

/// Why a removal run stopped.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UninstallError {
    /// One directory does not match its marker. Nothing was removed anywhere.
    Refused {
        dir: PathBuf,
        reasons: Vec<RefusalReason>,
    },
    /// A file or directory could not be read, inspected, or removed.
    Io { detail: String },
}

/// Remove the recorded skill package for `record` and print one line per package directory. `None` targets the portable root.
///
/// # Errors
/// A directory refuses, or a file system call fails. The error is the plain message the caller prints.
pub(crate) fn uninstall_skill_only_and_report(
    record: Option<&HarnessRecord>,
    env: &DirectWriteEnv,
) -> Result<(), String> {
    let outcomes = uninstall_skill_only(record, env).map_err(|error| failure_message(&error))?;
    println!("{}", outcome_text(&outcomes));
    Ok(())
}

/// Remove the recorded skill package for every root `record` reads, or the portable root when `record` is `None`.
///
/// Every directory is inspected first, so a refusal returns before any directory is changed. A directory that holds no package, or an empty directory that holds no marker, becomes a no-op; a non-empty directory with no marker refuses. A package that another harness still names is retained and its marker drops the harness this run removed.
///
/// # Errors
/// A skill root cannot be resolved, a path below the home is a symbolic link, a directory refuses, or a file system call fails.
pub(crate) fn uninstall_skill_only(
    record: Option<&HarnessRecord>,
    env: &DirectWriteEnv,
) -> Result<Vec<RemovalOutcome>, UninstallError> {
    let harness = record.map(|record| record.name);
    let home = env.home().ok_or_else(|| UninstallError::Io {
        detail: String::from("cannot resolve a skill root because the home directory is unknown"),
    })?;
    let mut inspected = Vec::new();
    for dir in package_dirs(record, env)? {
        if let Some(link) = ancestor_symlink(home, &dir)? {
            return Err(refused(
                &dir,
                RefusalReason::Symlink {
                    path: link.display().to_string(),
                },
            ));
        }
        inspected.push(inspect_dir(&dir)?);
    }
    let mut outcomes = Vec::with_capacity(inspected.len());
    for entry in inspected {
        outcomes.push(match entry {
            Inspected::NoOp { dir } => RemovalOutcome::NoOp { dir },
            Inspected::Ready { dir, marker } => remove_owned(&dir, &marker, harness)?,
        });
    }
    Ok(outcomes)
}

/// One directory's inspected state, decided before any directory is changed.
enum Inspected {
    /// The package directory or its marker is absent.
    NoOp { dir: PathBuf },
    /// The package matches its marker and may be removed or retained.
    Ready { dir: PathBuf, marker: SkillMarker },
}

/// The marker a package directory holds.
enum FoundMarker {
    /// No marker file is there.
    Absent,
    /// The marker path or its bytes are unusable.
    Refused(RefusalReason),
    /// The marker the file holds.
    Read(SkillMarker),
}

/// Inspect one package directory and either report a no-op, refuse with every reason, or return it ready for removal.
fn inspect_dir(dir: &Path) -> Result<Inspected, UninstallError> {
    let metadata = match std::fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Inspected::NoOp {
                dir: dir.to_path_buf(),
            });
        }
        Err(error) => return Err(io_failure("inspect", dir, &error)),
    };
    if metadata.file_type().is_symlink() {
        let path = dir.display().to_string();
        return Err(refused(dir, RefusalReason::Symlink { path }));
    }
    let marker = match read_marker(dir) {
        FoundMarker::Absent => return inspect_markerless(dir),
        FoundMarker::Refused(reason) => return Err(refused(dir, reason)),
        FoundMarker::Read(marker) => marker,
    };
    if marker.installer != SKILL_INSTALLER {
        return Err(refused(
            dir,
            RefusalReason::ForeignInstaller {
                recorded: marker.installer,
            },
        ));
    }
    let reasons = inspect(dir, &marker)?;
    if !reasons.is_empty() {
        return Err(UninstallError::Refused {
            dir: dir.to_path_buf(),
            reasons,
        });
    }
    Ok(Inspected::Ready {
        dir: dir.to_path_buf(),
        marker,
    })
}

/// What a package directory with no marker means: an empty directory is a no-op, and any entry is refused as a package an earlier clio installed without a record of what it owns.
fn inspect_markerless(dir: &Path) -> Result<Inspected, UninstallError> {
    let on_disk = scan_package_dir(dir).map_err(write_failure)?;
    if on_disk.is_empty() {
        return Ok(Inspected::NoOp {
            dir: dir.to_path_buf(),
        });
    }
    let reasons = on_disk
        .keys()
        .map(|path| RefusalReason::Conflict(ConflictReason::EarlierInstall { path: path.clone() }))
        .collect();
    Err(UninstallError::Refused {
        dir: dir.to_path_buf(),
        reasons,
    })
}

/// The marker `dir` holds, if any. A marker this program cannot read is a refusal, because without it nothing proves which files clio owns.
fn read_marker(dir: &Path) -> FoundMarker {
    let path = dir.join(SKILL_MARKER_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return FoundMarker::Refused(RefusalReason::Symlink {
                path: SKILL_MARKER_FILE.to_owned(),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FoundMarker::Absent,
        Err(_) => return FoundMarker::Refused(unreadable_marker()),
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FoundMarker::Absent,
        Err(_) => return FoundMarker::Refused(unreadable_marker()),
    };
    match SkillMarker::from_bytes(&bytes) {
        Ok(marker) => FoundMarker::Read(marker),
        Err(_) => FoundMarker::Refused(unreadable_marker()),
    }
}

/// The one reason for a marker whose bytes this program cannot use.
fn unreadable_marker() -> RefusalReason {
    RefusalReason::Conflict(ConflictReason::UnreadableMarker {
        path: SKILL_MARKER_FILE.to_owned(),
    })
}

/// Every fact that stops `dir` being removed: a symbolic link on a recorded path, a missing or edited file, an unrecorded file, or a payload digest that does not match the recorded bytes.
///
/// The symbolic-link check runs before the directory scan, because the scan reads through links. The payload digest is compared only when every recorded hash matches, so one edited file is reported as the edit alone.
fn inspect(dir: &Path, marker: &SkillMarker) -> Result<Vec<RefusalReason>, UninstallError> {
    let mut reasons = Vec::new();
    for relative in marker.files.keys() {
        if symlink_component(dir, relative)? {
            reasons.push(RefusalReason::Symlink {
                path: relative.clone(),
            });
        }
    }
    if !reasons.is_empty() {
        return Ok(reasons);
    }
    let mut found: Vec<(&str, Vec<u8>)> = Vec::with_capacity(marker.files.len());
    for (relative, recorded) in &marker.files {
        let path = dir.join(relative);
        match std::fs::read(&path) {
            Ok(bytes) => {
                if sha256_hex(&bytes) != *recorded {
                    reasons.push(RefusalReason::Conflict(ConflictReason::LocalEdit {
                        path: relative.clone(),
                    }));
                }
                found.push((relative.as_str(), bytes));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                reasons.push(RefusalReason::Conflict(ConflictReason::MissingOwned {
                    path: relative.clone(),
                }));
            }
            Err(error) => return Err(io_failure("read", &path, &error)),
        }
    }
    let on_disk = scan_package_dir(dir).map_err(write_failure)?;
    for path in on_disk.keys() {
        if path != SKILL_MARKER_FILE && !marker.files.contains_key(path) {
            reasons.push(RefusalReason::Conflict(ConflictReason::UnexpectedFile {
                path: path.clone(),
            }));
        }
    }
    if reasons.is_empty() {
        let bytes: Vec<(&str, &[u8])> = found
            .iter()
            .map(|(name, bytes)| (*name, bytes.as_slice()))
            .collect();
        let digest = payload_digest_of_bytes(&bytes);
        if digest != marker.payload_digest {
            reasons.push(RefusalReason::PayloadDigest {
                recorded: marker.payload_digest.clone(),
                found: digest,
            });
        }
    }
    reasons.sort_by(|left, right| left.path().cmp(right.path()));
    Ok(reasons)
}

/// Remove `dir` or retain it for another harness.
///
/// The recorded harness set drops `harness` when one is named. A non-empty remainder retains every asset and rewrites the marker with that remainder, so the shared copy keeps naming the harnesses that still read it. Otherwise only the recorded files, their now-empty parent directories strictly inside the package directory, and the marker are deleted, and the package directory goes only when this installation created it and it holds no entry.
fn remove_owned(
    dir: &Path,
    marker: &SkillMarker,
    harness: Option<&str>,
) -> Result<RemovalOutcome, UninstallError> {
    let mut remaining = marker.harnesses.clone();
    let dropped = harness.is_some_and(|name| remaining.remove(name));
    if !remaining.is_empty() {
        if dropped {
            let updated = SkillMarker {
                harnesses: remaining.clone(),
                ..marker.clone()
            };
            write_marker(dir, &updated)?;
        }
        return Ok(RemovalOutcome::Retained {
            dir: dir.to_path_buf(),
            remaining,
        });
    }
    let mut files = Vec::with_capacity(marker.files.len());
    let mut parents: BTreeSet<PathBuf> = BTreeSet::new();
    for relative in marker.files.keys() {
        let path = dir.join(relative);
        std::fs::remove_file(&path).map_err(|error| io_failure("remove", &path, &error))?;
        if let Some(parent) = path.parent().filter(|parent| *parent != dir) {
            let _ = parents.insert(parent.to_path_buf());
        }
        files.push(relative.clone());
    }
    let mut ordered: Vec<&PathBuf> = parents.iter().collect();
    ordered.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for parent in ordered {
        let mut current = Some(parent.as_path());
        while let Some(candidate) = current {
            if candidate == dir
                || !candidate.starts_with(dir)
                || std::fs::remove_dir(candidate).is_err()
            {
                break;
            }
            current = candidate.parent();
        }
    }
    let marker_path = dir.join(SKILL_MARKER_FILE);
    std::fs::remove_file(&marker_path)
        .map_err(|error| io_failure("remove", &marker_path, &error))?;
    let removed_dir = remove_created_dir(dir, marker)?;
    Ok(RemovalOutcome::Removed {
        dir: dir.to_path_buf(),
        files,
        removed_dir,
    })
}

/// Remove the package directory when this installation created it and it is now empty.
fn remove_created_dir(dir: &Path, marker: &SkillMarker) -> Result<bool, UninstallError> {
    if !marker.created_package_dir {
        return Ok(false);
    }
    let mut entries = std::fs::read_dir(dir).map_err(|error| io_failure("read", dir, &error))?;
    if entries.next().is_some() {
        return Ok(false);
    }
    std::fs::remove_dir(dir).map_err(|error| io_failure("remove", dir, &error))?;
    Ok(true)
}

/// Rewrite the marker after a harness leaves the recorded set.
fn write_marker(dir: &Path, marker: &SkillMarker) -> Result<(), UninstallError> {
    let bytes = marker.to_bytes().map_err(|error| UninstallError::Io {
        detail: format!(
            "failed to serialize the skill marker for {}: {error:?}",
            dir.display()
        ),
    })?;
    write_bytes_atomic(&dir.join(SKILL_MARKER_FILE), &bytes).map_err(write_failure)
}

/// One refusal for one directory.
fn refused(dir: &Path, reason: RefusalReason) -> UninstallError {
    UninstallError::Refused {
        dir: dir.to_path_buf(),
        reasons: vec![reason],
    }
}

fn write_failure(error: WriteError) -> UninstallError {
    match error {
        WriteError::Io { detail } => UninstallError::Io { detail },
        WriteError::Unparseable { path, snippet } => UninstallError::Io {
            detail: format!("{} {snippet}", path.display()),
        },
    }
}

#[cfg(test)]
#[path = "skill_uninstall_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "skill_uninstall_refusal_tests.rs"]
mod refusal_tests;
