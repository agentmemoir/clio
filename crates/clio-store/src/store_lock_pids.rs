// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for naming the process that holds the store
//! lock, so a refusal can point the operator at the process to stop: the pid
//! an exclusive hold records in the lock file, and the flock holders the
//! platform reports.
//!
//! ## Owns
//! - `recorded_pid`: the pid line an exclusive hold keeps in the lock file
//! - `flock_holder_pids` / `holder_pids`: the contending pids reported for
//!   the lock file, where the platform reports them
//! - The `/proc/locks` entry parser the lookup reads
//!
//! ## Does not own
//! - The lock itself and its hold types (see `store_lock`)
//! - The wording of a refusal (see `store_lock`)
//!
//! ## Boundary
//!
//! Changes to how a holder's pid is found should normally require changes
//! here. Changes to when the lock is taken should not.
//!
//! Keep this module focused on naming the lock's holder.

use std::path::Path;

// The inode accessor exists to match a `/proc/locks` entry against the lock
// file, and `/proc/locks` is Linux's alone. Importing it unconditionally makes
// it an unused import on every other Unix, which fails a `-D warnings` build.
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;

/// The pids whose `flock` on the lock file has `mode` (`READ` or `WRITE`),
/// where the platform reports them. Linux lists flock holders with their pid
/// in `/proc/locks`; elsewhere the list is empty and the refusal names the
/// lock file alone. When `recorded_fallback` is set, a pid the exclusive hold
/// recorded in the lock file is used when the platform reports nothing, since
/// that record is kept current by a live exclusive hold.
pub(crate) fn holder_pids(lock_path: &Path, mode: &str, recorded_fallback: bool) -> Vec<u32> {
    let mut pids = flock_holder_pids(lock_path, mode);
    if pids.is_empty()
        && recorded_fallback
        && let Some(pid) = recorded_pid(lock_path)
    {
        pids.push(pid);
    }
    pids
}

/// One `/proc/locks` entry's fields the holder lookup needs.
#[cfg(target_os = "linux")]
struct LocksEntry {
    mode: String,
    pid: u32,
    dev: (u64, u64),
    inode: u64,
}

/// Parse one `/proc/locks` line into its holder's mode, pid, device, and
/// inode. A line that is not a flock entry, or that lacks one of the fields
/// this lookup reads, is not a holder and yields `None` rather than a guess.
#[cfg(target_os = "linux")]
fn parse_locks_line(line: &str) -> Option<LocksEntry> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    // One entry reads `1: FLOCK ADVISORY WRITE 162502 00:26:160885 0 EOF`:
    // the pid precedes the device, printed in hex, and the inode.
    if fields.len() < 6 || fields[1] != "FLOCK" {
        return None;
    }
    let device: Vec<&str> = fields[5].split(':').collect();
    if device.len() != 3 {
        return None;
    }
    Some(LocksEntry {
        mode: fields[3].to_string(),
        pid: fields[4].parse::<u32>().ok()?,
        dev: (
            u64::from_str_radix(device[0], 16).ok()?,
            u64::from_str_radix(device[1], 16).ok()?,
        ),
        inode: device[2].parse::<u64>().ok()?,
    })
}

/// The flock holders `/proc/locks` reports for the lock file's device and
/// inode, or an empty list off Linux.
#[cfg(target_os = "linux")]
fn flock_holder_pids(lock_path: &Path, mode: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_to_string("/proc/locks") else {
        return Vec::new();
    };
    let Ok(metadata) = std::fs::metadata(lock_path) else {
        return Vec::new();
    };
    let (major, minor) = dev_parts(metadata.dev());
    entries
        .lines()
        .filter_map(parse_locks_line)
        .filter(|entry| {
            entry.mode == mode && entry.dev == (major, minor) && entry.inode == metadata.ino()
        })
        .map(|entry| entry.pid)
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn flock_holder_pids(_lock_path: &Path, _mode: &str) -> Vec<u32> {
    Vec::new()
}

/// The pid an exclusive hold recorded in the lock file, when the file holds a
/// current one. The file is empty or `pid <pid>`; anything else is treated as
/// no record rather than guessed at.
pub(crate) fn recorded_pid(lock_path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(lock_path).ok()?;
    let pid = text.trim().strip_prefix("pid ")?.trim();
    pid.parse::<u32>().ok()
}

/// The major and minor device numbers of a `stat` device id, using the
/// 64-bit encoding with 32-bit major and minor numbers.
#[cfg(target_os = "linux")]
fn dev_parts(dev: u64) -> (u64, u64) {
    let major = ((dev & 0x0000_0000_000f_ff00) >> 8) | ((dev & 0xffff_f000_0000_0000) >> 32);
    let minor = (dev & 0xff) | ((dev & 0x0fff_fff0_0000) >> 12);
    (major, minor)
}

#[cfg(test)]
#[path = "store_lock_pids_tests.rs"]
mod tests;
