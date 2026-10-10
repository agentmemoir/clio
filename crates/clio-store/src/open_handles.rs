// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for counting the file-backed store handles this
//! process currently holds, so a restore can refuse to replace a file that is
//! still open.
//!
//! ## Owns
//! - `OpenHandle`, the registration one open store handle holds for its
//!   lifetime
//! - `register` and `open_handles`, the count keyed by canonical path
//!
//! ## Does not own
//! - The swap that consults the count (see `restore_swap`)
//! - Store open and bootstrap (see `sqlite`) or the key store (see
//!   `local_kms_db`)
//!
//! ## Boundary
//!
//! Changes to how open handles are counted should normally require changes
//! here. Changes to what a restore replaces should not.
//!
//! Keep this module focused on counting open store handles.
//!
//! # Why a count exists
//!
//! SQLite resolves a database's `-wal` and `-shm` sidecars by file name, not by
//! file descriptor. A connection left open on the old database would, after the
//! restore's rename, open the sidecar that now belongs to the new file and can
//! flush the old database's cached log pages into it. Counting open handles is
//! how the swap proves the process closed everything before it touches the
//! first sidecar. The count is process-local: another process's handle cannot
//! be seen from here. The cross-process store lock (see `store_lock`) sits in
//! front of it: a restore refuses other processes' holds through the lock and
//! this process's through the count.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Open handle counts, keyed by the canonical path of the file they hold.
fn registry() -> &'static Mutex<HashMap<PathBuf, usize>> {
    static OPEN: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    OPEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The registry key for a path: the canonical parent plus the file name, so a
/// path that does not exist yet keys the same before and after it is created,
/// and a symlinked directory is still one directory. A path with no parent is
/// used as given. The store lock derives its file from the same key, so the
/// restore and every open name the same lock file.
pub(crate) fn key(path: &Path) -> PathBuf {
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    match parent.and_then(|dir| std::fs::canonicalize(dir).ok()) {
        Some(parent) => parent.join(name),
        None => path.to_path_buf(),
    }
}

/// One open handle's registration. Dropping it releases the count, so the
/// count follows the handle's real lifetime rather than a bookkeeping call.
#[derive(Debug)]
pub(crate) struct OpenHandle {
    path: PathBuf,
}

impl Drop for OpenHandle {
    fn drop(&mut self) {
        let mut open = registry().lock().expect("open-handle registry poisoned");
        if let Some(count) = open.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                open.remove(&self.path);
            }
        }
    }
}

/// Register one open handle on `path`.
pub(crate) fn register(path: &Path) -> OpenHandle {
    let key = key(path);
    let mut open = registry().lock().expect("open-handle registry poisoned");
    *open.entry(key.clone()).or_insert(0) += 1;
    OpenHandle { path: key }
}

/// How many open handles this process holds on `path`.
pub(crate) fn open_handles(path: &Path) -> usize {
    registry()
        .lock()
        .expect("open-handle registry poisoned")
        .get(&key(path))
        .copied()
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "open_handles_tests.rs"]
mod tests;
