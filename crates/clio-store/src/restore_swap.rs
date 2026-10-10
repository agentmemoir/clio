// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite swap: replacing one live store
//! file with a staged, verified payload and, when the archive carries one, the
//! live key store with the archive's key member.
//!
//! ## Owns
//! - `SwapRequest` / `KeyPlacement` / `SwapReport` and the fixed stage order
//! - The sidecar rule: every connection closed, sidecars removed, then the
//!   rename, then the directory fsync — for the store and for the key store
//! - The key replacement and the partial-restore report
//!
//! ## Does not own
//! - Producing or extracting the staged payload (see `clio-backup` and
//!   `sqlite_snapshot`)
//! - Deciding whether a restore may run (see `clio`)
//! - The free-space pre-flight (see `restore_space`) or the target measurement
//!   (see `restore_state`)
//!
//! ## Boundary
//!
//! Changes to how a restore places a file should normally require changes
//! here. Changes to what an archive carries should not.
//!
//! Keep this module focused on the swap.
//!
//! # The fixed order
//!
//! `close every connection`, `stage`, `verify`, `remove sidecars`, `rename`,
//! `fsync`, `swap the key`. The restore takes the store's cross-process lock
//! exclusively before all of it and releases it after the post-rename
//! measurement, so no other process can open the store mid-swap. The sidecars
//! are removed **before** the rename on
//! purpose: SQLite replays a sidecar it finds beside a database, so a crash
//! between the two would otherwise leave the old log to be applied to the new
//! file. A connection left open is the same hazard reached by another route,
//! because SQLite resolves sidecar names rather than descriptors: the store
//! lock refuses another process's hold, and the process-local open-handle
//! count refuses this process's own — a hold the lock cannot be taken past
//! (see `store_lock`).
//!
//! The key store gets the same treatment: it is a SQLite database with its own
//! journal, so its sidecars go before it is renamed over, not after.
//!
//! One direction of that ordering does cost something, and it is the only one:
//! a crash *between* removing the sidecars and the rename leaves the old store
//! without any transaction that was still only in its write-ahead log. That
//! window is small and the ordering is still the least-bad choice, but a
//! pre-restore snapshot is the only thing that covers it, so `--no-pre-backup`
//! accepts that risk rather than removing it.

use crate::migrate::map_db_err;
use crate::restore_key_swap::place_staged_key;
use clio_types::{AmError, ErrorCode};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The key store a restore places, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPlacement {
    /// The staged key member; must sit in the target's directory.
    pub staged: PathBuf,
    /// The live key store the staged file replaces.
    pub destination: PathBuf,
}

/// One restore's placement request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapRequest {
    /// The staged, digest-verified payload. Must sit in the target's directory
    /// so the rename stays on one filesystem.
    pub staged_payload: PathBuf,
    /// The live store file to replace.
    pub target: PathBuf,
    /// The archive's key store, when the archive carries one.
    pub key: Option<KeyPlacement>,
}

/// What a swap observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapReport {
    /// Bytes of the placed database.
    pub bytes: u64,
    /// Sidecars removed before the rename, in removal order.
    pub sidecars_removed: Vec<PathBuf>,
    /// Whether the target's directory was fsynced after the rename.
    pub directory_synced: bool,
    /// Whether the key store was replaced.
    pub key_replaced: bool,
    /// The step that failed after the database was replaced, when one did. The
    /// store names it so the report says which step, rather than the caller
    /// inferring it from which field is set.
    pub failed_step: Option<&'static str>,
    /// Why `failed_step` failed. Carries both problems when the key swap and
    /// the measurement both failed.
    pub failure: Option<String>,
    /// Every user table of the restored store, with its row count. Empty when
    /// the measurement failed.
    pub table_counts: Vec<(String, u64)>,
    /// The distinct bank ids of the restored store, ordered by bank id. Empty
    /// when the measurement failed.
    pub banks: Vec<String>,
}

impl SwapReport {
    /// Whether the database was replaced but the run did not fully complete:
    /// the key store was not replaced, or the replaced store could not be
    /// measured afterwards.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        self.failed_step.is_some()
    }

    /// The row count of one table of the restored store.
    #[must_use]
    pub fn count_of(&self, table: &str) -> Option<u64> {
        self.table_counts
            .iter()
            .find(|(name, _)| name == table)
            .map(|(_, count)| *count)
    }
}

/// The step name reported when the key store was not replaced.
pub const FAILED_STEP_KEY: &str = "key-store swap";

/// The step name reported when the replaced store could not be measured.
pub const FAILED_STEP_MEASURE: &str = "measuring the restored store";

/// The sidecar suffixes a SQLite database can leave beside itself: the
/// write-ahead log and its index, and the rollback journal this build's own
/// store writes. All three are removed before the rename; leaving the journal
/// behind is the same defect as leaving the log behind, because SQLite replays
/// whichever one it finds.
const SIDECAR_SUFFIXES: &[&str] = &["-wal", "-shm", "-journal"];

/// Where a test run stops the swap, to prove the target survives a crash
/// between stages. Production always passes [`FaultPoint::None`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    None,
    #[cfg(test)]
    AfterSidecars,
    #[cfg(test)]
    AfterRename,
}

/// Place a staged payload over its target, then the staged key store.
///
/// # Errors
/// The staged payload is not beside the target, another process holds the
/// store's lock, this process still holds an open handle on a file being
/// replaced, the staged payload is not a valid
/// database, a sidecar cannot be removed, or the rename fails. Nothing is
/// replaced on any of those paths.
///
/// A failure *after* the rename is reported through [`SwapReport::failed_step`]
/// rather than returned, because by then the store really has been replaced and
/// the caller has to tell the operator that, along with where the pre-restore
/// snapshot is.
pub fn swap_sqlite_store(request: &SwapRequest) -> Result<SwapReport, AmError> {
    swap_with_stop(request, FaultPoint::None)
}

/// [`swap_sqlite_store`] with a test-only stop point.
fn swap_with_stop(request: &SwapRequest, stop: FaultPoint) -> Result<SwapReport, AmError> {
    // The stop point is only compared in test builds; production always passes
    // `FaultPoint::None` and never reads it.
    #[cfg(not(test))]
    let _ = stop;
    // Same directory, so the rename cannot cross a filesystem and the staged
    // payload never sits in a shared temporary directory.
    refuse_not_beside(&request.staged_payload, &request.target)?;
    if let Some(key) = &request.key {
        refuse_not_beside(&key.staged, &request.target)?;
    }
    // The cross-process hold is the outer guard: it refuses while any other
    // process holds the store, before the first sidecar is touched. A hold
    // this process already has cannot be taken past, so the in-process count
    // below refuses it precisely instead.
    if crate::store_lock::this_process_holds_shared(&request.target) {
        refuse_open_handles(&request.target, request.key.as_ref())?;
    }
    // A named binding: `let _ = ...` would drop the hold immediately and
    // remove the guard for the rest of the swap.
    let _exclusive = crate::store_lock::ExclusiveHold::acquire(&request.target)?;
    // The whole safety argument rests on nothing flushing the old file after
    // the rename, so the count is checked before the first sidecar is touched.
    refuse_open_handles(&request.target, request.key.as_ref())?;
    // The payload must be a real database before anything is replaced; this is
    // the store's own check, called on the staged file.
    crate::sqlite_integrity_ok(&request.staged_payload)?;
    if let Some(key) = &request.key {
        refuse_not_a_key_store(&key.staged)?;
    }
    let bytes = file_bytes(&request.staged_payload)?;
    let sidecars_removed = remove_sidecars(&request.target)?;
    #[cfg(test)]
    if stop == FaultPoint::AfterSidecars {
        return Err(stopped("before the rename"));
    }
    std::fs::rename(&request.staged_payload, &request.target).map_err(|error| {
        map_db_err(
            &format!(
                "placing the staged payload over {}",
                request.target.display()
            ),
            error,
        )
    })?;
    let directory_synced = sync_parent(&request.target);
    #[cfg(test)]
    if stop == FaultPoint::AfterRename {
        return Err(stopped("after the rename"));
    }
    let (key_replaced, key_failure) = replace_key(request.key.as_ref());
    // The store has already been replaced, so a failure to measure it is
    // reported rather than returned. Returning it would discard the whole
    // report, and with it the pre-restore snapshot id, which is the operator's
    // only way back after a restore that did happen. A competing process
    // holding the new store is enough to make this measurement fail.
    let (failed_step, failure, table_counts, banks) =
        match crate::restore_state::sqlite_file_state(&request.target) {
            Ok(state) => (
                key_failure.as_ref().map(|_| FAILED_STEP_KEY),
                key_failure,
                state.table_counts,
                state.banks,
            ),
            Err(error) => (
                Some(FAILED_STEP_MEASURE),
                Some(unmeasured(&request.target, &error, key_failure)),
                Vec::new(),
                Vec::new(),
            ),
        };
    Ok(SwapReport {
        bytes,
        sidecars_removed,
        directory_synced,
        key_replaced,
        failed_step,
        failure,
        table_counts,
        banks,
    })
}

/// The report's failure text when the replaced store could not be measured,
/// carrying the key failure alongside it when both steps failed.
fn unmeasured(target: &Path, error: &AmError, key_failure: Option<String>) -> String {
    let measured = format!(
        "the restored store at {} was replaced but could not be measured: {}",
        target.display(),
        error.message
    );
    match key_failure {
        Some(key) => format!("{measured}; and {key}"),
        None => measured,
    }
}

/// The error a test-only stop point returns, worded as what it simulates.
#[cfg(test)]
fn stopped(where_: &str) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("the restore was interrupted {where_} (simulated crash)"),
    )
}

/// Refuse a staged file that is not in the target's own directory.
fn refuse_not_beside(staged: &Path, target: &Path) -> Result<(), AmError> {
    let staged_dir = canonical_parent(staged)?;
    let target_dir = canonical_parent(target)?;
    if staged_dir == target_dir {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::InvalidArgument,
        format!(
            "the staged file {} is not in the target's directory {}; the rename that places it \
             must stay on one filesystem",
            staged.display(),
            target_dir.display()
        ),
    ))
}

/// Refuse while this process still holds an open store handle on a file the
/// swap would replace.
fn refuse_open_handles(target: &Path, key: Option<&KeyPlacement>) -> Result<(), AmError> {
    let paths = std::iter::once(target).chain(key.map(|placement| placement.destination.as_path()));
    for path in paths {
        let open = crate::open_handles::open_handles(path);
        if open > 0 {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "refusing to replace {} while {open} open store handle(s) still point at it: \
                     SQLite resolves its sidecar files by name, so a handle left open can flush \
                     the old log into the new file; close every store connection and retry",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Refuse a staged key file whose head is not a SQLite key-store database.
pub(crate) fn refuse_not_a_key_store(staged: &Path) -> Result<(), AmError> {
    const MAGIC: &[u8] = b"SQLite format 3\0";
    let mut head = [0u8; 16];
    let mut file = std::fs::File::open(staged).map_err(|error| {
        map_db_err(
            &format!("opening the staged key store {}", staged.display()),
            error,
        )
    })?;
    let read = file.read(&mut head).map_err(|error| {
        map_db_err(
            &format!("reading the staged key store {}", staged.display()),
            error,
        )
    })?;
    if read < head.len() || head != MAGIC {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "the staged key store {} is not a SQLite key-store database",
                staged.display()
            ),
        ));
    }
    Ok(())
}

/// Remove the target's sidecars, returning the paths that existed.
pub(crate) fn remove_sidecars(target: &Path) -> Result<Vec<PathBuf>, AmError> {
    let mut removed = Vec::new();
    for suffix in SIDECAR_SUFFIXES {
        let path = sidecar_path(target, suffix);
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(map_db_err(
                    &format!("removing the sidecar {}", path.display()),
                    error,
                ));
            }
        }
    }
    Ok(removed)
}

/// The sidecar path for one suffix, built from the OS string so a path that is
/// not valid UTF-8 still names its sidecar correctly.
fn sidecar_path(target: &Path, suffix: &str) -> PathBuf {
    let mut os = target.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// Fsync the target's directory so the rename is durable.
///
/// A directory that cannot be opened or flushed is reported as
/// `directory_synced: false` rather than a failure: the rename is complete and
/// the store is the new one, and only a crash in the next instant could lose
/// the directory entry.
fn sync_parent(target: &Path) -> bool {
    let Some(parent) = target.parent().filter(|dir| !dir.as_os_str().is_empty()) else {
        return false;
    };
    let Ok(dir) = std::fs::File::open(parent) else {
        return false;
    };
    dir.sync_all().is_ok()
}

/// The canonical parent directory of a path, refusing a path with no parent.
fn canonical_parent(path: &Path) -> Result<PathBuf, AmError> {
    let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) else {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{} has no directory to place it in", path.display()),
        ));
    };
    Ok(std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()))
}

/// The size of a staged file.
fn file_bytes(path: &Path) -> Result<u64, AmError> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| map_db_err(&format!("measuring {}", path.display()), error))
}

/// Replace the live key store with the staged one, reporting a failure instead
/// of returning it: the database swap has already happened, so the outcome is
/// a partial restore that the caller must report, never a plain error.
fn replace_key(key: Option<&KeyPlacement>) -> (bool, Option<String>) {
    let Some(key) = key else {
        return (false, None);
    };
    let report = place_staged_key(&key.staged, &key.destination);
    (report.replaced, report.failure)
}

#[cfg(test)]
#[path = "restore_swap_tests.rs"]
pub(crate) mod tests;

/// The key-only swap's own tests.
/// The swap's failure arms, each reached deliberately.
#[cfg(test)]
#[path = "restore_swap_fault_tests.rs"]
mod fault_tests;

/// The swap's store-lock tests.
#[cfg(test)]
#[path = "restore_swap_lock_tests.rs"]
mod lock_tests;
