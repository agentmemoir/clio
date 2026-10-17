// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the free-space gate a verify pass runs before
//! it stages the payload beside the archive.
//!
//! ## Owns
//! - `require_room`: the refusal when the staging directory cannot hold the
//!   payload the manifest records
//! - The POSIX `df` lookup and report parser that answer the free-space question
//!
//! ## Does not own
//! - Staging or removing the payload file (see `staging`)
//! - The archive walk that decides what to stage (see `verify`)
//! - Restore's own pre-flight, which plans several members at once (see
//!   `clio-store`)
//!
//! ## Boundary
//!
//! Changes to how much room a verify pass needs should normally require changes
//! here. Changes to when a member is staged should not.
//!
//! Keep this module focused on one free-space question.

use clio_types::{AmError, ErrorCode};
use std::path::Path;

/// The absolute paths `df` is looked for, in order: `/bin/df` is macOS and
/// BusyBox, `/usr/bin/df` is the merged-`/usr` Linux, NixOS, and Gentoo layout,
/// and the `sbin` pair covers hosts that split `sbin` out of `usr`.
///
/// `PATH` is deliberately absent. The check is a gate, so the program that
/// answers it is fixed here rather than searched for.
const DF_PATHS: [&str; 4] = ["/bin/df", "/usr/bin/df", "/usr/sbin/df", "/sbin/df"];

/// Refuse when `dir` cannot hold `bytes` of payload.
///
/// # Errors
/// `QuotaExceeded` when the filesystem reports less free space than the
/// payload needs. `Internal` when `df` is missing, fails, or reports nothing
/// readable: a check that could not run must not answer "there is room".
pub(crate) fn require_room(dir: &Path, bytes: u64) -> Result<(), AmError> {
    if bytes == 0 {
        return Ok(());
    }
    let available = available_bytes(dir)?;
    if available < bytes {
        return Err(AmError::new(
            ErrorCode::QuotaExceeded,
            format!(
                "staging the payload needs {bytes} bytes in {}, but only {available} bytes are \
                 free there",
                dir.display()
            ),
        ));
    }
    Ok(())
}

/// The filesystem's available bytes for `dir`, from the POSIX `df` report.
fn available_bytes(dir: &Path) -> Result<u64, AmError> {
    run_available_bytes(dir, &df_output)
}

/// Run one `df` and read its report.
///
/// `run` is injected so every failure arm is testable without breaking the
/// host's `df`.
fn run_available_bytes(
    dir: &Path,
    run: &dyn Fn(&Path) -> std::io::Result<std::process::Output>,
) -> Result<u64, AmError> {
    let output = run(dir).map_err(|error| {
        AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot check free space in {}: `df` failed: {error}",
                dir.display()
            ),
        )
    })?;
    if !output.status.success() {
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "cannot check free space in {}: `df` exited with {}",
                dir.display(),
                output.status
            ),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_available_kb(&text)
        .map(|kb| kb.saturating_mul(1024))
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::Internal,
                format!(
                    "cannot check free space in {}: `df` printed no readable available-space \
                     column",
                    dir.display()
                ),
            )
        })
}

/// Build and run `df -P -k <dir>` against the absolute `df` this host provides.
fn df_output(dir: &Path) -> std::io::Result<std::process::Output> {
    let program = DF_PATHS
        .iter()
        .map(std::path::PathBuf::from)
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "no `df` at any of these absolute paths: {}",
                    DF_PATHS.join(", ")
                ),
            )
        })?;
    std::process::Command::new(program)
        .arg("-P")
        .arg("-k")
        .arg(dir)
        .output()
}

/// The available-space column of a POSIX `df -P -k` report: the header line is
/// skipped and the second line's fourth column is the count in 1024-byte
/// blocks.
fn parse_available_kb(text: &str) -> Option<u64> {
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

#[cfg(test)]
#[path = "free_space_tests.rs"]
mod tests;
