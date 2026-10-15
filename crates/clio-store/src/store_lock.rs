// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the cross-process advisory whole-file hold
//! on the lock derived from a path. A store open takes it shared and a restore
//! takes it exclusive, so a restore refuses while any process holds the store.
//! The same hold is taken exclusively on the erasure journal path, so a
//! restore placement and an erasure denial share one lock.
//!
//! ## Owns
//! - `SharedHold` and `ExclusiveHold`, their bounded wait and their release
//!   on drop
//! - The lock file: its path beside the path it is given, its owner-only
//!   creation, and the pid the exclusive hold records in it
//! - The process-local hold counts that make the restore's own opens
//!   coexist with its exclusive hold
//!
//! ## Does not own
//! - The process-local open-handle registry (see `open_handles`), the inner
//!   check a restore runs behind this lock
//! - The swap that takes the exclusive hold (see `restore_swap`)
//! - The erasure journal and what an erasure denial does under the hold
//! - SQLite's own file locking and Postgres's advisory locks
//!
//! ## Boundary
//!
//! Changes to when a path is locked or refused should normally require changes
//! here. Changes to what a restore replaces or an erasure denies should not.
//!
//! Keep this module focused on the cross-process file hold.
//!
//! # How the lock works
//!
//! The lock is one advisory `flock` on `<path>.lock`, beside the path it is
//! derived from, taken through the standard library's file-lock calls. The OS
//! releases the hold when the process exits, so a lock file left behind by a
//! dead process never blocks a later run: the next process locks the same file
//! again, and a stale file is harmless. Shared holds coexist with each other;
//! a hold that would contend waits a bounded time and then refuses, always
//! naming the lock and the direction of the contention, and naming the holding
//! process where the platform reports one. Linux lists flock holders with
//! their pid in `/proc/locks`; elsewhere the list is empty. The exclusive
//! hold records its own pid in the lock file, so a store open refused during
//! a restore names the restoring process on every platform. Naming a
//! *shared* holder is best effort: the pid list is whatever `/proc/locks`
//! reports for the lock file, and the refusal is complete and correct when
//! it names none, because the platform may list no entry or the file may not
//! be readable. The lock is advisory: it guards a restore's swap, it does
//! not make the store multi-process safe for concurrent writes, and a
//! process that opens the database without going through a store open path
//! is not covered.

use crate::migrate::map_db_err;
use crate::store_lock_pids::holder_pids;
use clio_types::{AmError, ErrorCode};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a hold waits for a contending hold before refusing.
///
/// A restore's swap takes well under this on a healthy system, so an ordinary
/// store open during a swap waits out the swap instead of failing it.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// How often a waiting hold retries its non-blocking attempt.
const LOCK_RETRY: Duration = Duration::from_millis(25);

/// The suffix appended to the store's file name for its lock file.
const LOCK_SUFFIX: &str = ".lock";

/// This process's holds on one lock file.
#[derive(Default)]
struct HoldCounts {
    shared: usize,
    exclusive: bool,
}

/// This process's holds, keyed by lock file.
///
/// A `flock` lives on the open file description, so two descriptors of one
/// process contend with each other. The restore needs the opposite twice:
/// its own read-only opens must go through while it holds the exclusive
/// hold, and a store it still has open must be refused precisely, by the
/// open-handle count, rather than vaguely by the lock. So a shared hold taken
/// while this process holds the exclusive one is granted without a second
/// flock, and an exclusive hold is refused up front when this process already
/// holds a shared one.
fn holds() -> &'static Mutex<HashMap<PathBuf, HoldCounts>> {
    static HOLDS: OnceLock<Mutex<HashMap<PathBuf, HoldCounts>>> = OnceLock::new();
    HOLDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The lock file for `store`: the store's canonical parent plus its name and
/// `.lock`, by the same rule the open-handle registry keys a path, so the
/// restore and every open derive the same file from the same store.
pub(crate) fn lock_file_path(store: &Path) -> PathBuf {
    let mut key = crate::open_handles::key(store);
    key.as_mut_os_string().push(LOCK_SUFFIX);
    key
}

/// A shared hold on a store. Every store open takes one for the lifetime of
/// its handle; it is released on drop.
#[derive(Debug)]
pub(crate) struct SharedHold {
    lock_path: PathBuf,
    /// The locked descriptor. `None` for a hold granted inside this
    /// process's own exclusive hold, which needs no second flock.
    file: Option<File>,
}

impl SharedHold {
    /// Take a shared hold on the store's lock, waiting a bounded time when a
    /// restore holds the store exclusively.
    ///
    /// # Errors
    /// The lock file cannot be opened, or the lock stayed exclusively held
    /// for the whole bounded wait.
    pub(crate) fn acquire(store: &Path) -> Result<Self, AmError> {
        Self::acquire_with_wait(store, LOCK_WAIT)
    }

    pub(crate) fn acquire_with_wait(store: &Path, wait: Duration) -> Result<Self, AmError> {
        let lock_path = lock_file_path(store);
        if exclusive_by_this_process(&lock_path) {
            // This process holds the store exclusively — the restore swap's
            // own opens run inside it — so the shared hold needs no second
            // flock: no other process can hold the store meanwhile.
            let mut holds = holds().lock().expect("store-lock registry poisoned");
            let counts = holds.entry(lock_path.clone()).or_default();
            counts.shared += 1;
            return Ok(Self {
                lock_path,
                file: None,
            });
        }
        let file = open_lock_file(&lock_path, false)?;
        try_bounded(&file, wait, Mode::Shared, store, &lock_path)?;
        let mut holds = holds().lock().expect("store-lock registry poisoned");
        holds.entry(lock_path.clone()).or_default().shared += 1;
        drop(holds);
        Ok(Self {
            lock_path,
            file: Some(file),
        })
    }
}

impl Drop for SharedHold {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            // Closing the descriptor releases the hold too; the explicit
            // unlock makes the release visible here rather than at an
            // unspecified close.
            let _ = file.unlock();
        }
        let mut holds = holds().lock().expect("store-lock registry poisoned");
        if let Some(counts) = holds.get_mut(&self.lock_path) {
            counts.shared = counts.shared.saturating_sub(1);
            if counts.shared == 0 && !counts.exclusive {
                holds.remove(&self.lock_path);
            }
        }
    }
}

/// An exclusive hold on the lock derived from a path: refused while any
/// process holds that lock shared, released on drop. A restore takes it on the
/// store; a denial takes it on the erasure journal, so both share one lock.
#[derive(Debug)]
pub struct ExclusiveHold {
    lock_path: PathBuf,
    file: File,
}

impl ExclusiveHold {
    /// Take the lock derived from `path` exclusively, waiting a bounded time
    /// for every shared hold, in every process, to clear.
    ///
    /// # Errors
    /// This process already holds the lock shared or exclusively, the lock
    /// file cannot be opened for writing, or another process held the lock
    /// for the whole bounded wait.
    pub fn acquire(path: &Path) -> Result<Self, AmError> {
        Self::acquire_with_wait(path, LOCK_WAIT)
    }

    pub(crate) fn acquire_with_wait(path: &Path, wait: Duration) -> Result<Self, AmError> {
        let lock_path = lock_file_path(path);
        {
            let holds = holds().lock().expect("store-lock registry poisoned");
            let counts = holds.get(&lock_path);
            if counts.is_some_and(|counts| counts.exclusive) {
                return Err(AmError::new(
                    ErrorCode::Internal,
                    format!(
                        "the store lock {} is already held exclusively by this process",
                        lock_path.display()
                    ),
                ));
            }
            if counts.is_some_and(|counts| counts.shared > 0) {
                // The restore refuses this case through the open-handle
                // count, which names the leaked handle precisely; the lock
                // cannot be taken past this process's own shared holds.
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "refusing to take the store lock {} exclusively while this process holds \
                         {} shared hold(s) on the store",
                        lock_path.display(),
                        counts.map_or(0, |counts| counts.shared)
                    ),
                ));
            }
        }
        let file = open_lock_file(&lock_path, true)?;
        try_bounded(&file, wait, Mode::Exclusive, path, &lock_path)?;
        record_pid(&file);
        let mut holds = holds().lock().expect("store-lock registry poisoned");
        holds.entry(lock_path.clone()).or_default().exclusive = true;
        drop(holds);
        Ok(Self { lock_path, file })
    }
}

impl Drop for ExclusiveHold {
    fn drop(&mut self) {
        // The recorded pid goes with the hold, so a later refusal never
        // names a process that no longer holds the store.
        let _ = self.file.set_len(0);
        let _ = self.file.unlock();
        let mut holds = holds().lock().expect("store-lock registry poisoned");
        if let Some(counts) = holds.get_mut(&self.lock_path) {
            counts.exclusive = false;
            if counts.shared == 0 {
                holds.remove(&self.lock_path);
            }
        }
    }
}

/// Whether this process holds any shared hold on `store`'s lock.
///
/// The restore consults this before taking its exclusive hold: a lock it
/// cannot take past this process's own shared holds is refused through the
/// open-handle count instead, which names the leaked handle precisely.
pub(crate) fn this_process_holds_shared(store: &Path) -> bool {
    let holds = holds().lock().expect("store-lock registry poisoned");
    holds
        .get(&lock_file_path(store))
        .is_some_and(|counts| counts.shared > 0)
}

/// Whether this process holds the exclusive hold on `lock_path`.
fn exclusive_by_this_process(lock_path: &Path) -> bool {
    holds()
        .lock()
        .expect("store-lock registry poisoned")
        .get(lock_path)
        .is_some_and(|counts| counts.exclusive)
}

/// Which lock mode a bounded attempt waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Shared,
    Exclusive,
}

impl Mode {
    /// The `flock` attempt this mode makes on one descriptor.
    fn try_on(self, file: &File) -> Result<(), std::fs::TryLockError> {
        match self {
            Mode::Shared => file.try_lock_shared(),
            Mode::Exclusive => file.try_lock(),
        }
    }

    /// The pids the platform reports for this direction's contending holders.
    ///
    /// The lookup is best effort. A shared hold refused by a restore falls
    /// back to the pid the exclusive hold records in the lock file, which a
    /// live exclusive holder keeps current, so that direction names the
    /// restoring process even when the platform reports nothing. An exclusive
    /// hold refused by shared holders uses only the platform's flock listing:
    /// a restore that died mid-swap leaves its own pid line behind, and a
    /// shared holder records nothing, so there is no record to fall back to.
    fn reported_holders(self, lock_path: &Path) -> Vec<u32> {
        match self {
            Mode::Shared => holder_pids(lock_path, "WRITE", true),
            Mode::Exclusive => holder_pids(lock_path, "READ", false),
        }
    }

    /// The refusal for a hold that waited out its bounded wait.
    fn refused(self, store: &Path, lock_path: &Path) -> AmError {
        self.refusal(store, lock_path, &self.reported_holders(lock_path))
    }

    /// The refusal text for a holder list, split from the platform lookup so
    /// both branches are unit-testable without a live second process.
    ///
    /// The lock and the direction of the contention are always named. The pids
    /// are best effort: an empty list adds nothing, because the lookup may
    /// find no `/proc/locks` entry or may not read the file. Given a list, the
    /// message names every pid in order.
    fn refusal(self, store: &Path, lock_path: &Path, holders: &[u32]) -> AmError {
        let held = match self {
            Mode::Shared => "held exclusively",
            Mode::Exclusive => "held shared",
        };
        let named = match holders {
            [] => String::new(),
            holders => format!(
                " by pid(s) {}",
                holders
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        match self {
            Mode::Shared => AmError::new(
                ErrorCode::Internal,
                format!(
                    "the store at {} cannot be opened because a restore is replacing it: the \
                     store lock {} is {held}{named}; wait for the restore to finish and retry",
                    store.display(),
                    lock_path.display()
                ),
            ),
            Mode::Exclusive => AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "refusing to replace the store at {} while another process holds it: the \
                     store lock {} is {held}{named}; stop the process or processes holding the \
                     store and retry",
                    store.display(),
                    lock_path.display()
                ),
            ),
        }
    }
}

/// Retry a non-blocking lock attempt until it is granted or `wait` runs out.
fn try_bounded(
    file: &File,
    wait: Duration,
    mode: Mode,
    store: &Path,
    lock_path: &Path,
) -> Result<(), AmError> {
    let deadline = Instant::now() + wait;
    loop {
        match mode.try_on(file) {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(map_db_err(
                    &format!("locking the store lock {}", lock_path.display()),
                    error,
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
        }
        if Instant::now() >= deadline {
            return Err(mode.refused(store, lock_path));
        }
        std::thread::sleep(LOCK_RETRY);
    }
}

/// Open the lock file, creating it owner-only when missing.
///
/// A shared hold never needs write access, so when the store's directory does
/// not permit creating the file, the hold is still taken through a read-only
/// descriptor of an existing lock file. An exclusive hold must record its
/// pid, so it requires the writable open.
fn open_lock_file(lock_path: &Path, require_writable: bool) -> Result<File, AmError> {
    let context = format!("opening the store lock {}", lock_path.display());
    let writable = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path);
    match writable {
        Ok(file) => Ok(file),
        Err(error) if !require_writable => OpenOptions::new()
            .read(true)
            .open(lock_path)
            .map_err(|_| map_db_err(&context, error)),
        Err(error) => Err(map_db_err(&context, error)),
    }
}

/// Record this process's pid in the lock file, so a store open refused while
/// this exclusive hold is live can name the restoring process. The file is
/// truncated first, so a pid a reader sees belongs to the current holder. The
/// record is best effort: the lock works without it, and the refusal then
/// names the lock file alone.
fn record_pid(file: &File) {
    if let Ok(mut writer) = file.try_clone() {
        let _ = writer.set_len(0);
        let _ = writeln!(writer, "pid {}", std::process::id());
        let _ = writer.flush();
    }
}

#[cfg(test)]
#[path = "store_lock_tests.rs"]
mod tests;

/// The lock's own two-process tests: a process-local test cannot demonstrate
/// a cross-process guarantee, so contention is exercised through a real
/// second process.
#[cfg(test)]
#[path = "store_lock_process_tests.rs"]
mod process_tests;
