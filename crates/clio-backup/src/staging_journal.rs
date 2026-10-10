// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sidecar journal a restore keeps beside
//! the members it stages: the paths this process staged, the process that
//! staged them, and the removal of what a process that is gone left behind.
//!
//! ## Owns
//! - `StagingJournal`: the owner-only journal one restore records each staged
//!   path in, and that removes itself with the value that owns it
//! - `sweep_staging_journals` and `JournalSweepReport`: what a restore found in
//!   a directory before it staged anything of its own
//! - `OwnerProbe` and the procfs answer behind it: whether the process that
//!   recorded a path is still running
//!
//! ## Does not own
//! - The name rules that tell a journal from a staged member (see
//!   `staging_journal_name`)
//! - Staging one member, and removing it when the process returns normally (see
//!   `staging`)
//! - Which members an archive declares and what they contain (see `extract`)
//! - Running a restore (see the CLI half)
//!
//! ## Boundary
//!
//! Changes to what the journal records, to which recorded owner may be swept,
//! or to when a staged path may be deleted, should normally require changes
//! here. Changes to how a member is staged, or to what it is named, should not.
//!
//! Keep this module focused on the record of a restore's staged paths, not on
//! staging them.

/// The name family a journal and a staged member share, and the two rules the
/// sweep recognises them by.
#[path = "staging_journal_name.rs"]
mod staging_journal_name;

use crate::staging::{create_owner_only, io_error};
use clio_types::AmError;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) use staging_journal_name::{
    JOURNAL_SUFFIX, JOURNAL_TAG, STAGING_PREFIX, is_journal_name, is_staged_name,
};
// Only the tests name the staged suffix, so it is re-exported for them alone, to
// keep reaching it through `use super::*`.
#[cfg(test)]
pub(crate) use staging_journal_name::STAGED_SUFFIX;

/// Name attempts before a journal gives up on finding a free name.
const NAME_ATTEMPTS: u32 = 8;

/// One staged file, and the process that created it.
///
/// The whole record is a name, a pid, and that pid's start time. No member
/// name, size, digest, or byte of the payload is written here, so the journal is
/// never a second copy of what a restore stages — the key member, which is key
/// material, is named and nothing more.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct JournalEntry {
    /// The staged file's name inside the directory the journal and the staged
    /// file share. A name rather than a path on purpose, and one the sweep
    /// requires to be a single path component, so a journal cannot point the
    /// sweep at a file outside the directory it is sweeping, however it came to
    /// be edited.
    name: String,
    /// The process that created the staged file.
    pid: u32,
    /// That process's start time in clock ticks since boot, where the host
    /// reports one. It is what tells a recycled pid from the process that
    /// recorded the entry, so a dead owner is still recognised after its pid
    /// has been handed to another process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_ticks: Option<u64>,
}

/// One restore's sidecar journal: the staged paths this process created, in the
/// directory it created them in.
///
/// The file is named from the same `clio-snapshot-` family a staged member uses
/// and carries this process's pid, so two concurrent restores never write one
/// file and never need a lock on it. It holds one JSON object per line, because
/// a process killed mid-append leaves a torn last line, and a line-based reader
/// skips that one entry instead of discarding every entry before it.
#[derive(Debug)]
pub(crate) struct StagingJournal {
    /// The journal's own path, removed when this value is dropped.
    path: PathBuf,
    /// The open journal. Each entry is one `write`, so an entry written before
    /// a member is streamed is already in the kernel by the time the streaming
    /// starts, and a process that dies part-way through still leaves the record
    /// behind for the next restore.
    file: std::fs::File,
    /// This process's pid, recorded with every entry.
    pid: u32,
    /// This process's start time, recorded with every entry where the host
    /// reports one.
    start_ticks: Option<u64>,
}

impl StagingJournal {
    /// Arm a journal for this process in `dir`, or `None` when none can be
    /// armed.
    ///
    /// `None` is not a refusal: the staged files are still removed on every path
    /// out of a restore that returns, and the journal exists only to catch a
    /// process that does not. The names are tried in the same
    /// `reserve_path` shape, so a name another attempt in this process took is
    /// stepped over rather than mistaken for a real failure.
    pub(crate) fn create(dir: &Path) -> Option<Self> {
        let pid = std::process::id();
        let start_ticks = self_start_ticks();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        for attempt in 0..NAME_ATTEMPTS {
            let path = dir.join(format!(
                "{STAGING_PREFIX}{JOURNAL_TAG}-{pid}-{nanos}-{attempt}{JOURNAL_SUFFIX}"
            ));
            if path.exists() {
                continue;
            }
            // Owner-only from the first byte, like a staged member. The journal
            // names the paths a restore is writing beside the operator's
            // archives, which is not a place another account reads from.
            let Ok(file) = create_owner_only(&path) else {
                return None;
            };
            return Some(Self {
                path,
                file,
                pid,
                start_ticks,
            });
        }
        None
    }

    /// Where this journal's own file lives.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Record the staged file `path` in the journal.
    ///
    /// # Errors
    /// The journal cannot be appended to.
    pub(crate) fn record(&mut self, path: &Path) -> Result<(), AmError> {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let entry = JournalEntry {
            name,
            pid: self.pid,
            start_ticks: self.start_ticks,
        };
        let mut line = serde_json::to_string(&entry)
            .map_err(|error| io_error("writing a journal entry", &std::io::Error::other(error)))?;
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .map_err(|error| io_error("appending to a staging journal", &error))
    }
}

impl Drop for StagingJournal {
    fn drop(&mut self) {
        // The journal is this process's own record and this process is finished
        // with it: the staged files it names are owned, and removed, by whoever
        // staged them.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Record the staged file `path` in this run's journal, arming the journal on
/// first use.
///
/// A journal that cannot be armed is not an error, for the reason
/// [`StagingJournal::create`] gives. An append that fails *is* one: the caller
/// has to remove the staged file rather than leave it on disk with no record of
/// who owned it.
///
/// # Errors
/// The journal exists and cannot be appended to.
pub(crate) fn record_staged(
    journal: &mut Option<StagingJournal>,
    dir: &Path,
    path: &Path,
) -> Result<(), AmError> {
    if journal.is_none() {
        *journal = StagingJournal::create(dir);
    }
    match journal.as_mut() {
        Some(open) => open.record(path),
        None => Ok(()),
    }
}

/// What one sweep of a directory found.
///
/// The counts are the whole report: this runs before a restore stages anything,
/// so a directory that cannot be read is not a restore failure, and the restore
/// itself reports the member it could not stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalSweepReport {
    /// Journal files read.
    pub journals_examined: usize,
    /// Staged files removed, because the process that created them is gone.
    pub staged_files_removed: usize,
    /// Journals left where they were: because an owner is alive, because the
    /// host could not say whether it is, or because a staged file the journal
    /// named could not be removed and the next sweep has to try again.
    pub journals_left_alone: usize,
}

/// Sweep the staging journals in `dir`: remove the staged files whose owning
/// process is gone, then the journals that named them.
///
/// This runs at the start of a restore, before that restore stages anything,
/// which is the only moment a staged path in `dir` cannot belong to a process
/// that is still writing it. A journal is swept only when every entry it holds
/// names a process proven gone, and only a file whose name is a staged member's
/// own single-component name is ever removed, so neither an entry nor a planted
/// file can point this at a path outside `dir`. Anything else — an archive, a
/// manifest, another journal, a name that only looks like a staged one — is left
/// alone. A journal is removed only once every staged file it named is gone, so a
/// removal that failed is retried by the next sweep instead of being forgotten.
///
/// Nothing here is reported as an error. A journal that cannot be read, or a
/// staged file that cannot be removed, is a cleanup that did not finish, and
/// every one of those outcomes leaves a file on disk rather than removing a
/// live owner's staged payload, so the safe direction is the one that happens.
pub fn sweep_staging_journals(dir: &Path) -> JournalSweepReport {
    sweep_with(dir, &ProcfsProbe)
}

/// Sweep `dir` with `probe` deciding liveness, which is what makes the sweep's
/// two answers testable without a real process coming and going.
fn sweep_with(dir: &Path, probe: &dyn OwnerProbe) -> JournalSweepReport {
    let mut report = JournalSweepReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };
    for entry in entries.flatten() {
        if !is_journal_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        report.journals_examined += 1;
        sweep_one(&entry.path(), probe, &mut report);
    }
    report
}

/// Sweep one journal, or leave it alone.
fn sweep_one(path: &Path, probe: &dyn OwnerProbe, report: &mut JournalSweepReport) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        report.journals_left_alone += 1;
        return;
    };
    let entries: Vec<JournalEntry> = raw
        .lines()
        // A line that does not parse is a torn or edited line. It is skipped,
        // not fatal: the entries around it are still good, and a skipped entry
        // can only ever leak its staged file.
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    // A journal with no usable entry is left alone, whatever the probe says.
    // There is no owner left to prove gone, and this process's own journal has
    // no entry until it stages something, so sweeping it would be sweeping a
    // live restore's journal.
    let proven_gone = !entries.is_empty()
        && entries
            .iter()
            .all(|entry| entry.pid != std::process::id() && probe.state(entry) == OwnerState::Gone);
    if !proven_gone {
        report.journals_left_alone += 1;
        return;
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut every_name_gone = true;
    for entry in &entries {
        if !is_staged_name(&entry.name) {
            continue;
        }
        match std::fs::remove_file(dir.join(&entry.name)) {
            Ok(()) => report.staged_files_removed += 1,
            // A target that is already gone meets the goal: retrying would leak
            // the journal itself for a file that is not there.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                report.staged_files_removed += 1;
            }
            // Any other failure leaves the journal for the next sweep, because
            // the file it names may still be on disk.
            Err(_) => every_name_gone = false,
        }
    }
    if !every_name_gone {
        report.journals_left_alone += 1;
        return;
    }
    // The journal has nothing left to say: its owner is gone and the paths it
    // named have been dealt with.
    let _ = std::fs::remove_file(path);
}

/// What the sweep knows about the process that recorded an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerState {
    /// The recorded process is running, and it is the one that recorded the
    /// entry. Nothing it staged may be swept.
    // Off Linux the probe has no per-process directory to read, so it answers
    // `Undetermined` and nothing in the library constructs this state there.
    // The variant stays on every platform because it is part of the state the
    // sweep speaks, not a Linux detail, and the tests build it anywhere.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(dead_code, reason = "only the Linux procfs probe constructs it")
    )]
    Alive,
    /// The recorded process is gone: its pid is not in use, or the process at
    /// that pid started later and is a different one.
    Gone,
    /// The host cannot say. Nothing may be swept, because a live owner's staged
    /// file — or a staged key store, which is key material — must never be
    /// deleted on a guess.
    Undetermined,
}

/// How the sweep decides whether a recorded owner is still running.
pub(crate) trait OwnerProbe {
    /// The state of the process that recorded `entry`.
    fn state(&self, entry: &JournalEntry) -> OwnerState;
}

/// The procfs answer: `/proc/<pid>` exists exactly while that process does.
///
/// One limit is worth stating plainly. With `hidepid` the kernel hides another
/// user's pid exactly as it hides a dead one, so a false `Gone` needs a staged
/// file owned by a different uid — and the owner-only staging directory and the
/// owner-only staged files already exclude that. Nothing here proves more than
/// "this pid is not there with the start time the entry recorded".
#[cfg(target_os = "linux")]
struct ProcfsProbe;

#[cfg(target_os = "linux")]
impl OwnerProbe for ProcfsProbe {
    fn state(&self, entry: &JournalEntry) -> OwnerState {
        let raw = match std::fs::read_to_string(format!("/proc/{}/stat", entry.pid)) {
            Ok(raw) => raw,
            // Match the error this read actually returned. `last_os_error` is
            // whatever the last syscall in this process left behind, which is
            // not necessarily this one.
            Err(error) => {
                return match error.kind() {
                    // A missing directory is the absence of the process only on
                    // a host whose procfs this process has confirmed. Without
                    // that, a `NotFound` could be a boot without procfs, and
                    // the safe direction is the one that leaves the file.
                    std::io::ErrorKind::NotFound if procfs_is_readable() => OwnerState::Gone,
                    // A boot without procfs, a permission refusal, a name the
                    // host will not decode: none of these is evidence about a
                    // process, so none of them is a licence to sweep.
                    _ => OwnerState::Undetermined,
                };
            }
        };
        let Some(recorded) = entry.start_ticks else {
            // A journal from a host that reports no start time is honoured
            // conservatively: a pid that is present is taken to be the owner, so
            // nothing is swept.
            return OwnerState::Alive;
        };
        match start_ticks(&raw) {
            // The same start time is the same process, so it owns these files.
            Some(current) if current == recorded => OwnerState::Alive,
            // A different start time at the same pid is a recycled pid. The
            // owner that wrote this entry is gone either way.
            Some(_) => OwnerState::Gone,
            // A `stat` that does not parse says nothing about who is running.
            None => OwnerState::Undetermined,
        }
    }
}

/// Whether this host's procfs is present and readable, answered once.
///
/// A `NotFound` on another process's directory is the absence of that process
/// only if there is a procfs for it to be absent from, so the sweep reads its
/// own `/proc/self/stat` once per process and remembers the answer. A host with
/// no readable procfs then sweeps nothing at all.
#[cfg(target_os = "linux")]
fn procfs_is_readable() -> bool {
    static READABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *READABLE.get_or_init(|| {
        std::fs::read_to_string("/proc/self/stat").is_ok_and(|raw| start_ticks(&raw).is_some())
    })
}

/// Off Linux there is no per-process directory to read, so liveness cannot be
/// determined and every journal is left alone. The direction is the safe one: a
/// staged file left on disk, rather than a live owner's staged payload deleted
/// because the host could not answer.
#[cfg(not(target_os = "linux"))]
struct ProcfsProbe;

#[cfg(not(target_os = "linux"))]
impl OwnerProbe for ProcfsProbe {
    fn state(&self, _entry: &JournalEntry) -> OwnerState {
        OwnerState::Undetermined
    }
}

/// This process's start time, where the host reports one.
fn self_start_ticks() -> Option<u64> {
    let raw = std::fs::read_to_string(format!("/proc/{}/stat", std::process::id())).ok()?;
    start_ticks(&raw)
}

/// Field 22 of `/proc/<pid>/stat`: the process's start time in clock ticks
/// since boot.
///
/// The `comm` field is parenthesised and may itself contain spaces and
/// parentheses, so the split is on the RIGHTMOST `)`: an earlier one belongs to
/// a name, not to the end of the field. The fields after it begin at field 3,
/// the process state, so field 22 is the twentieth of them.
fn start_ticks(stat: &str) -> Option<u64> {
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_ascii_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
#[path = "staging_journal_tests.rs"]
mod tests;

/// The sweep against real processes: one that is still running, and one that
/// was killed. Linux only, because it needs a real pid and a real `/proc`.
#[cfg(all(test, target_os = "linux"))]
#[path = "staging_journal_process_tests.rs"]
mod process_tests;
