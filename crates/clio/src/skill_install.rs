// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for installing the embedded `clio-mcp` skill package from the binary into every skill root the selected harness reads.
//!
//! ## Owns
//! - Resolving every skill root for a harness through the injectable `DirectWriteEnv` seam
//! - Reading each package directory and its provenance marker, so every root is planned before any root is written
//! - Writing the planned assets, then the marker last, creating the directories it needs
//! - Writing the marker alone when only its own recorded fields are stale, and adopting a package an earlier clio installed with no marker
//! - Leaving an already-current root untouched: no asset write, no marker write, no timestamp change
//! - The skill outcome value: written, unchanged, or refused with every conflict reason
//!
//! ## Does not own
//! - The embedded asset table (`clio_mcp::skill_assets`)
//! - What the marker records or how its digest is taken (`skill_marker`)
//! - The decision between no-op, write, and conflict (`skill_install_plan`)
//! - The outcome text and the failure messages (`skill_install_text`)
//! - Which root a harness uses (`agent_harness`) or joining it to the home directory (`agent_harness_targets`)
//! - Registering the MCP server, which runs after this step
//!
//! ## Boundary
//!
//! Changes to where skill assets are written, in what order, or when a root is left alone should normally require changes here. Changes to the asset text belong in `skills/clio-mcp`, changes to the harness registry belong in `agent_harness`, and changes to the words printed belong in `skill_install_text`.
//!
//! Every root is planned from a scan and a marker read before the first byte is written, so a conflict in one root cannot leave another root half written.
//!
//! Keep this module focused on writing the embedded skill package.

use crate::agent_harness::{self, HarnessRecord, SkillRoot};
use crate::agent_harness_targets::{DirectWriteEnv, resolve_skill_root};
use crate::agent_harness_write::WriteError;
use crate::skill_install_plan::{
    ConflictReason, InstallPlan, plan_install, scan_package_dir, temp_sibling_name,
};
use crate::skill_install_text::{
    failure_message, outcome_text, portable_failure_message, portable_outcome_text, refused,
};
use crate::skill_marker::{self, SKILL_MARKER_FILE, SkillMarker};
use clio_mcp::skill_assets::{SKILL_ASSETS, references};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The directory name of the installed skill package. It matches the `name` in the router frontmatter, as the Agent Skills format requires.
pub(crate) const SKILL_DIR_NAME: &str = "clio-mcp";

/// The portable root alone, used when `clio install` names no harness.
const PORTABLE_ROOTS: &[SkillRoot] = &[SkillRoot::Portable];

/// One root's result: what it wrote, and what it kept.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SkillRootOutcome {
    /// The absolute package directory, `<root>/clio-mcp`.
    pub(crate) dir: PathBuf,
    /// Every package-relative path written in this root, in write order, with the marker last. An empty list means the root was already current.
    pub(crate) written: Vec<String>,
    /// The unowned paths inside the package directory, kept exactly as they were.
    pub(crate) preserved_foreign: Vec<String>,
}

/// One root that refused, with every reason it refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RootConflict {
    /// The absolute package directory that refused.
    pub(crate) dir: PathBuf,
    /// Every reason for the refusal, in path order.
    pub(crate) reasons: Vec<ConflictReason>,
}

/// What one installation run did. The caller distinguishes a no-op from a write from a refusal without comparing bytes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SkillOutcome {
    /// Every root already held the incoming payload and its marker: nothing was written in any root and no modification time changed.
    Unchanged { roots: Vec<PathBuf> },
    /// At least one root was written. A root that was already current appears with an empty `written` list.
    Written { roots: Vec<SkillRootOutcome> },
    /// At least one root refused, so nothing was written in any root.
    Refused { conflicts: Vec<RootConflict> },
}

/// Install every embedded skill asset for `record` and print the skill outcome text, one line per root. With `force`, a conflict is replaced instead of refused. Returns `Err` with a plain message when a root refuses or the write fails, and the caller must then skip registration.
pub(crate) fn install_and_report(
    record: &HarnessRecord,
    env: &DirectWriteEnv,
    force: bool,
) -> Result<(), String> {
    let outcome = install(record, env, force).map_err(|error| failure_message(record, &error))?;
    report(outcome_text(record, &outcome), &outcome)
}

/// Install the portable skill package with no harness selected, and print its outcome text. Returns `Err` with a plain message when a root refuses or the write fails, and the caller must then report the failure and skip registration.
pub(crate) fn install_portable_and_report(env: &DirectWriteEnv, force: bool) -> Result<(), String> {
    let outcome = install_portable(env, force).map_err(|error| portable_failure_message(&error))?;
    report(portable_outcome_text(&outcome), &outcome)
}

/// Plan every skill root `record` reads, then write the roots that need it. A refusal in any root leaves every root untouched.
///
/// # Errors
/// A skill root cannot be resolved, a package directory cannot be read, a directory cannot be created, or a file cannot be written.
pub(crate) fn install(
    record: &HarnessRecord,
    env: &DirectWriteEnv,
    force: bool,
) -> Result<SkillOutcome, WriteError> {
    let missing = format!(
        "cannot resolve a skill root for `{}` because the home directory is unknown",
        record.name
    );
    write_into_all(
        agent_harness::skill_roots(record.name),
        env,
        &missing,
        force,
        Some(record.name),
    )
}

/// Plan every portable skill root with no harness selected, then write the roots that need it.
///
/// # Errors
/// The home directory is unknown, a package directory cannot be read, a directory cannot be created, or a file cannot be written.
pub(crate) fn install_portable(
    env: &DirectWriteEnv,
    force: bool,
) -> Result<SkillOutcome, WriteError> {
    let missing = "cannot resolve the portable skill root because the home directory is unknown";
    write_into_all(PORTABLE_ROOTS, env, missing, force, None)
}

/// Plan every root, then write. Planning reads each package directory and its marker and runs no write, so a conflict in one root is known before another root is touched.
fn write_into_all(
    roots: &[SkillRoot],
    env: &DirectWriteEnv,
    missing: &str,
    force: bool,
    harness: Option<&str>,
) -> Result<SkillOutcome, WriteError> {
    let mut work = Vec::with_capacity(roots.len());
    let mut conflicts = Vec::new();
    for package in resolve_packages(roots, env, missing)? {
        match plan_package(&package, force, harness)? {
            Ok(plan) => work.push((package, plan)),
            Err(reasons) => conflicts.push(RootConflict {
                dir: package,
                reasons,
            }),
        }
    }
    if !conflicts.is_empty() {
        return Ok(SkillOutcome::Refused { conflicts });
    }
    if work
        .iter()
        .all(|(_, plan)| matches!(plan, RootWork::Current))
    {
        return Ok(SkillOutcome::Unchanged {
            roots: work.into_iter().map(|(package, _)| package).collect(),
        });
    }
    let mut roots = Vec::with_capacity(work.len());
    for (package, plan) in work {
        roots.push(match plan {
            RootWork::Current => SkillRootOutcome {
                dir: package,
                written: Vec::new(),
                preserved_foreign: Vec::new(),
            },
            RootWork::Write {
                files,
                preserved_foreign,
                marker,
            } => write_package(&package, &files, preserved_foreign, &marker)?,
        });
    }
    Ok(SkillOutcome::Written { roots })
}

/// Every package directory, `<root>/clio-mcp`, in mapping order. A root that resolves to a path already in the set is kept once, so a harness whose native root equals the portable root never gets a duplicate.
fn resolve_packages(
    roots: &[SkillRoot],
    env: &DirectWriteEnv,
    missing: &str,
) -> Result<Vec<PathBuf>, WriteError> {
    let mut packages: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let path = resolve_skill_root(*root, env).ok_or_else(|| WriteError::Io {
            detail: missing.to_owned(),
        })?;
        let package = path.join(SKILL_DIR_NAME);
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    Ok(packages)
}

/// One root's planned work, decided before any root is written.
enum RootWork {
    /// The root already holds the incoming payload and its marker.
    Current,
    /// These assets must be written. `preserved_foreign` is reported and kept.
    Write {
        files: Vec<String>,
        preserved_foreign: Vec<String>,
        /// The marker the write records, merged from the install state.
        marker: SkillMarker,
    },
}

/// The marker beside the assets in one package directory.
enum InstalledMarker {
    /// No marker file is there.
    Absent,
    /// The marker file is there and this program cannot read it.
    Unreadable,
    /// The marker the file holds.
    Read(SkillMarker),
}

/// Plan one package directory: scan it, read its marker, and let the plan decide.
///
/// The plan compares the marker on disk with the marker these assets produce, so a difference in the installer, the package version, the catalog hash, the payload digest, or the recorded file map is planned as a write even when every asset already matches.
///
/// An unreadable marker is its own conflict and is refused even with `force`: without the record of which files this program owns, no file's bytes can be attributed to it, so replacing anything would be a guess.
///
/// The expected marker carries the install state too: the recorded state when a marker is there, and otherwise whether this installation must create the package directory plus the `harness` that asked for it.
fn plan_package(
    package: &Path,
    force: bool,
    harness: Option<&str>,
) -> Result<Result<RootWork, Vec<ConflictReason>>, WriteError> {
    let on_disk = scan_package_dir(package)?;
    let marker = match installed_marker(package)? {
        InstalledMarker::Absent => None,
        InstalledMarker::Read(marker) => Some(marker),
        InstalledMarker::Unreadable => {
            return Ok(Err(vec![ConflictReason::UnreadableMarker {
                path: SKILL_MARKER_FILE.to_owned(),
            }]));
        }
    };
    let expected = SkillMarker::from_assets(SKILL_ASSETS, &skill_marker::catalog_hash())
        .with_install_state(marker.as_ref(), harness, package.exists());
    Ok(
        match plan_install(marker.as_ref(), &expected, &on_disk, force) {
            InstallPlan::Conflict { reasons } => Err(reasons),
            InstallPlan::NoOp => Ok(RootWork::Current),
            InstallPlan::Write {
                files,
                preserved_foreign,
            } => Ok(RootWork::Write {
                files,
                preserved_foreign,
                marker: expected,
            }),
        },
    )
}

/// The marker `package` holds, if any.
fn installed_marker(package: &Path) -> Result<InstalledMarker, WriteError> {
    let path = package.join(SKILL_MARKER_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstalledMarker::Absent);
        }
        Err(error) => return Err(io_error("read", &path, &error)),
    };
    Ok(match SkillMarker::from_bytes(&bytes) {
        Ok(marker) => InstalledMarker::Read(marker),
        Err(_) => InstalledMarker::Unreadable,
    })
}

/// Write the planned files into `package`, the marker last.
///
/// The references go first and the router goes last, so a failure on a reference leaves no loadable router that points at a missing file. Only the files the plan named are touched, so a plan holding nothing but the marker rewrites the marker and leaves every asset's bytes and timestamp alone. The marker written is the one the plan built from the incoming assets and the install state.
fn write_package(
    package: &Path,
    files: &[String],
    preserved_foreign: Vec<String>,
    marker: &SkillMarker,
) -> Result<SkillRootOutcome, WriteError> {
    std::fs::create_dir_all(package).map_err(|error| io_error("create", package, &error))?;
    let planned = |name: &str| files.iter().any(|file| file == name);
    let mut written = Vec::with_capacity(files.len() + 1);
    for (name, text) in references() {
        if planned(name) {
            write_asset(package, name, text)?;
            written.push((*name).to_owned());
        }
    }
    // The router is the first embedded asset; `references()` starts after it.
    let (name, text) = SKILL_ASSETS[0];
    if planned(name) {
        write_asset(package, name, text)?;
        written.push(name.to_owned());
    }
    if planned(SKILL_MARKER_FILE) && write_marker(package, marker)? {
        written.push(SKILL_MARKER_FILE.to_owned());
    }
    Ok(SkillRootOutcome {
        dir: package.to_path_buf(),
        written,
        preserved_foreign,
    })
}

/// Write one asset into `package`, creating the parent directory a reference needs.
fn write_asset(package: &Path, name: &str, text: &str) -> Result<(), WriteError> {
    let path = package.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| io_error("create", parent, &error))?;
    }
    write_bytes_atomic(&path, text.as_bytes())
}

/// Write `marker` unless the package already holds exactly those bytes. The plan already asked for it whenever a recorded field differs; the byte comparison stays as the last guard, so a marker that is somehow current is never rewritten. Returns whether it wrote, so the outcome names the marker only when the file changed.
fn write_marker(package: &Path, marker: &SkillMarker) -> Result<bool, WriteError> {
    let path = package.join(SKILL_MARKER_FILE);
    let bytes = marker.to_bytes().map_err(|error| WriteError::Io {
        detail: format!(
            "failed to serialize the skill marker for {}: {error:?}",
            path.display()
        ),
    })?;
    if std::fs::read(&path).is_ok_and(|existing| existing == bytes) {
        return Ok(false);
    }
    write_bytes_atomic(&path, &bytes)?;
    Ok(true)
}

/// Print `text`, or return it as the failure message when the outcome was a refusal. A refusal is not an I/O error, but it must still stop the command and fail it.
fn report(text: String, outcome: &SkillOutcome) -> Result<(), String> {
    if refused(outcome) {
        return Err(text);
    }
    println!("{text}");
    Ok(())
}

/// Replace `path` with `bytes` through a same-directory temp sibling and a rename. No backup is made: the assets are generated from the binary and a re-run restores them. A file whose bytes already match is never rewritten, which is what keeps a repeated install free of timestamp changes.
fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, bytes).map_err(|error| io_error("write", &tmp, &error))?;
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error("replace", path, &error));
    }
    Ok(())
}

/// A hidden temp sibling beside `path`, unique across threads and processes. The name comes from `skill_install_plan`, which the package scan reads to leave this program's own leftovers out of a plan.
fn temp_sibling(path: &Path) -> PathBuf {
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(temp_sibling_name(name, std::process::id(), nanos, seq))
}

fn io_error(action: &str, path: &Path, error: &std::io::Error) -> WriteError {
    WriteError::Io {
        detail: format!("failed to {action} {}: {error}", path.display()),
    }
}

#[cfg(test)]
#[path = "skill_install_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "skill_install_roots_tests.rs"]
mod roots_tests;

#[cfg(test)]
#[path = "skill_install_marker_tests.rs"]
mod marker_tests;

#[cfg(test)]
#[path = "skill_install_adoption_tests.rs"]
mod adoption_tests;

#[cfg(test)]
#[path = "skill_install_test_support.rs"]
mod test_support;
