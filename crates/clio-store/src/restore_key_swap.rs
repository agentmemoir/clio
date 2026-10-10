// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for replacing the key store with a staged key,
//! with or without a database swap: the validation, the sidecar rule, the
//! copy-then-swap fallback, and the outcome the caller reports.
//!
//! ## Owns
//! - `place_staged_key` and the `KeySwapReport` it returns
//! - The staged-then-swapped sequence for a key that does not already sit
//!   beside its destination
//!
//! ## Does not own
//! - The SQLite store swap that calls it (see `restore_swap`)
//! - The archive the staged key came from (see `clio-backup`)
//!
//! ## Boundary
//!
//! Changes to how a key store is replaced should normally require changes here.
//! Changes to the store swap sequence should not.
//!
//! Keep this module focused on replacing one key store.

use crate::restore_swap::{refuse_not_a_key_store, remove_sidecars};
use std::path::Path;

/// The outcome of a key-store swap: replaced, or the failure to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySwapReport {
    /// Whether the key store was replaced.
    pub replaced: bool,
    /// What stopped the replacement, when one did.
    pub failure: Option<String>,
}

impl KeySwapReport {
    /// Whether the swap did not happen.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        self.failure.is_some()
    }
}

/// Replace the live key store with the staged one, with or without a database
/// swap.
///
/// This is the key step of a restore that does not rename a database file: the
/// row load and the key swap are separate concerns, and the key member travels
/// exactly as it does for SQLite. The staged key is checked to be a SQLite
/// key-store database, the destination's sidecars go first, and a staged key
/// that does not already sit beside its destination is copied into that
/// directory (so the rename stays on one filesystem) before it is renamed over
/// the destination. The staged copy removes itself on every failure path, and a
/// failure is reported, not returned: the caller decides whether it is a
/// partial restore.
pub fn place_staged_key(staged: &Path, destination: &Path) -> KeySwapReport {
    let unreplaced = |reason: String| KeySwapReport {
        replaced: false,
        failure: Some(format!(
            "the key store at {} was not replaced: {reason}",
            destination.display()
        )),
    };
    if let Err(error) = refuse_not_a_key_store(staged) {
        return unreplaced(error.message);
    }
    // The same rule as the store, on the one file it would otherwise skip: the
    // key store is a SQLite database too, and SQLite replays a journal it finds
    // beside a database by name. A hot journal left by the old key store would
    // be rolled against the new file that takes its name.
    if let Err(error) = remove_sidecars(destination) {
        return unreplaced(error.message);
    }
    match std::fs::rename(staged, destination) {
        Ok(()) => KeySwapReport {
            replaced: true,
            failure: None,
        },
        // A rename across filesystems fails, so a staged key that does not
        // already sit in the destination's directory is copied there first and
        // renamed from there. The copy is owner-only and removes itself.
        Err(_) => copy_then_swap(staged, destination),
    }
}

/// Copy the staged key into the destination's directory, then rename it.
fn copy_then_swap(staged: &Path, destination: &Path) -> KeySwapReport {
    let Some(dir) = destination
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
    else {
        return KeySwapReport {
            replaced: false,
            failure: Some(format!(
                "the key store destination {} has no directory to stage the swap in",
                destination.display()
            )),
        };
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let temp = dir.join(format!(".clio-key-swap-{}-{nanos}", std::process::id()));
    if let Err(error) = std::fs::copy(staged, &temp) {
        return KeySwapReport {
            replaced: false,
            failure: Some(format!(
                "the key store could not be staged beside {}: {error}",
                dir.display()
            )),
        };
    }
    match std::fs::rename(&temp, destination) {
        Ok(()) => KeySwapReport {
            replaced: true,
            failure: None,
        },
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            KeySwapReport {
                replaced: false,
                failure: Some(format!(
                    "the key store at {} was not replaced: {error}",
                    destination.display()
                )),
            }
        }
    }
}

#[cfg(test)]
#[path = "restore_key_swap_tests.rs"]
mod restore_key_swap_tests;
