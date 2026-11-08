// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for installing the embedded `clio-mcp` skill package from the binary into every skill root the selected harness reads, and for rendering the skill outcome.
//!
//! ## Owns
//! - Resolving every skill root for a harness through the injectable `DirectWriteEnv` seam
//! - Writing every embedded asset into each `<root>/clio-mcp/`, creating the directories it needs
//! - Byte comparison so a re-run rewrites nothing that already matches, and a count of what changed
//! - The skill outcome line, one line per root, and the portable-root reason line
//!
//! ## Does not own
//! - The embedded asset table (`clio_mcp::skill_assets`)
//! - Which root a harness uses (`agent_harness`) or joining it to the home directory (`agent_harness_targets`)
//! - Registering the MCP server, which runs after this step
//!
//! ## Boundary
//!
//! Changes to where skill assets are written or how their outcome is reported should normally require changes here. Changes to the asset text belong in `skills/clio-mcp`, and changes to the harness registry belong in `agent_harness`.
//!
//! Keep this module focused on writing the embedded skill package.

use crate::agent_harness::{self, HarnessRecord, SkillRoot};
use crate::agent_harness_targets::{DirectWriteEnv, resolve_skill_root};
use crate::agent_harness_write::WriteError;
use clio_mcp::skill_assets::{SKILL_ASSETS, references};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The directory name of the installed skill package. It matches the `name` in the router frontmatter, as the Agent Skills format requires.
pub(crate) const SKILL_DIR_NAME: &str = "clio-mcp";

/// The portable root alone, used when `clio install` names no harness.
const PORTABLE_ROOTS: &[SkillRoot] = &[SkillRoot::Portable];

/// The hermes reason line: its client has no documented native skill mechanism, so it receives the portable copy.
const PORTABLE_REASON: &str =
    "note: hermes has no documented native skill mechanism, so the portable skill root is used";

/// What one target root received.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SkillRootOutcome {
    /// The absolute package directory, `<root>/clio-mcp`.
    pub dir: PathBuf,
    /// The number of files whose bytes were created or changed in this root. Zero means the root was already current.
    pub written: usize,
}

/// What a successful skill install left behind.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SkillOutcome {
    /// One entry per written root, in mapping order.
    pub roots: Vec<SkillRootOutcome>,
    /// The number of embedded assets in the package, the same for every root.
    pub assets: usize,
}

/// Install every embedded skill asset for `record` and print the skill outcome text, one line per root. Returns `Err` with a plain message when the write fails, and the caller must then skip registration.
pub(crate) fn install_and_report(
    record: &HarnessRecord,
    env: &DirectWriteEnv,
) -> Result<(), String> {
    let outcome = install(record, env).map_err(|error| failure_message(record, &error))?;
    println!("{}", outcome_text(record, &outcome));
    Ok(())
}

/// Install the portable skill package with no harness selected, and print its
/// outcome text. Returns `Err` with a plain message when the write fails, and
/// the caller must then report the failure and skip registration.
pub(crate) fn install_portable_and_report(env: &DirectWriteEnv) -> Result<(), String> {
    let outcome = install_portable(env).map_err(|error| portable_failure_message(&error))?;
    println!("{}", portable_outcome_text(&outcome));
    Ok(())
}

/// Write every embedded asset into every skill root `record` reads. A file whose bytes already match is left untouched, so a re-run is safe and reports zero written for that root.
///
/// # Errors
/// A skill root cannot be resolved, a directory cannot be created, or a file cannot be replaced.
pub(crate) fn install(
    record: &HarnessRecord,
    env: &DirectWriteEnv,
) -> Result<SkillOutcome, WriteError> {
    let missing = format!(
        "cannot resolve a skill root for `{}` because the home directory is unknown",
        record.name
    );
    write_into_all(agent_harness::skill_roots(record.name), env, &missing)
}

/// Write the embedded assets into the portable cross-client root. Used when
/// `clio install` names no harness.
///
/// # Errors
/// The home directory is unknown, a directory cannot be created, or a file
/// cannot be replaced.
pub(crate) fn install_portable(env: &DirectWriteEnv) -> Result<SkillOutcome, WriteError> {
    let missing = "cannot resolve the portable skill root because the home directory is unknown";
    write_into_all(PORTABLE_ROOTS, env, missing)
}

/// Resolve every target root and write the package into each. A root that
/// resolves to a path already in the set is written once, so a harness whose
/// native root equals the portable root never gets a duplicate write.
fn write_into_all(
    roots: &[SkillRoot],
    env: &DirectWriteEnv,
    missing: &str,
) -> Result<SkillOutcome, WriteError> {
    let mut dirs: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let path = resolve_skill_root(*root, env).ok_or_else(|| WriteError::Io {
            detail: missing.to_owned(),
        })?;
        if !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    let mut outcomes = Vec::with_capacity(dirs.len());
    for root in &dirs {
        outcomes.push(write_into(root)?);
    }
    Ok(SkillOutcome {
        roots: outcomes,
        assets: SKILL_ASSETS.len(),
    })
}

/// Write every embedded asset into `<root>/clio-mcp/`. The references go first
/// and the router goes last, so a failure on a reference leaves no loadable
/// router that points at a missing file.
fn write_into(root: &Path) -> Result<SkillRootOutcome, WriteError> {
    let dir = root.join(SKILL_DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|error| io_error("create", &dir, &error))?;
    let mut written = 0;
    for (name, text) in references() {
        written += write_asset(&dir, name, text)?;
    }
    // The router is the first embedded asset; `references()` starts after it.
    let (name, text) = SKILL_ASSETS[0];
    written += write_asset(&dir, name, text)?;
    Ok(SkillRootOutcome { dir, written })
}

/// Write one asset into `dir`, skipping it when its bytes already match.
fn write_asset(dir: &Path, name: &str, text: &str) -> Result<usize, WriteError> {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| io_error("create", parent, &error))?;
    }
    if std::fs::read(&path).is_ok_and(|bytes| bytes == text.as_bytes()) {
        return Ok(0);
    }
    write_bytes_atomic(&path, text.as_bytes())?;
    Ok(1)
}

/// The skill outcome text: one line per written root naming its absolute
/// directory and the asset count, plus whether anything changed there. A
/// harness that receives the portable copy also gets the reason line. The text
/// never claims a client loaded the skill.
pub(crate) fn outcome_text(record: &HarnessRecord, outcome: &SkillOutcome) -> String {
    let lines = outcome_lines(outcome);
    match portable_reason(record) {
        Some(reason) => format!("{lines}\n{reason}"),
        None => lines,
    }
}

/// The outcome text for the portable-only install, which names no harness.
pub(crate) fn portable_outcome_text(outcome: &SkillOutcome) -> String {
    outcome_lines(outcome)
}

/// One line per written root, so every directory is named.
fn outcome_lines(outcome: &SkillOutcome) -> String {
    outcome
        .roots
        .iter()
        .map(|root| root_line(root, outcome.assets))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The shared root line: the absolute directory and the asset count, plus
/// whether anything changed in that root.
fn root_line(root: &SkillRootOutcome, assets: usize) -> String {
    if root.written == 0 {
        format!(
            "skill assets already current in {} ({} assets, nothing written)",
            root.dir.display(),
            assets
        )
    } else {
        format!(
            "installed {} skill assets into {} ({} written)",
            assets,
            root.dir.display(),
            root.written
        )
    }
}

/// The plain failure message for a skill write: the harness, the failing path, and the cause. `WriteError` already carries the path in its detail.
pub(crate) fn failure_message(record: &HarnessRecord, error: &WriteError) -> String {
    match error {
        WriteError::Io { detail } => {
            format!(
                "could not install the {} skill assets: {detail}",
                record.name
            )
        }
        WriteError::Unparseable { path, snippet } => format!(
            "could not install the {} skill assets: {} {snippet}",
            record.name,
            path.display()
        ),
    }
}

/// The plain failure message for the portable-only write: the failing path and
/// the cause. `WriteError` already carries the path in its detail.
pub(crate) fn portable_failure_message(error: &WriteError) -> String {
    match error {
        WriteError::Io { detail } => {
            format!("could not install the portable clio-mcp skill assets: {detail}")
        }
        WriteError::Unparseable { path, snippet } => format!(
            "could not install the portable clio-mcp skill assets: {} {snippet}",
            path.display()
        ),
    }
}

/// The portable-copy reason for a harness that has no documented native skill mechanism.
fn portable_reason(record: &HarnessRecord) -> Option<&'static str> {
    (record.name == "hermes").then_some(PORTABLE_REASON)
}

/// Replace `path` with `bytes` through a same-directory temp sibling and a rename. No backup is made: the assets are generated from the binary and a re-run restores them.
fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, bytes).map_err(|error| io_error("write", &tmp, &error))?;
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error("replace", path, &error));
    }
    Ok(())
}

/// A hidden temp sibling beside `path`, unique across threads and processes.
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
    path.with_file_name(format!(".{name}.{}.{nanos}.{seq}.tmp", std::process::id()))
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
