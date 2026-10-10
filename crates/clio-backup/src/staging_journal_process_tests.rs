// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the sweep against real processes and
//! the real process table: the real `OwnerProbe`'s own answers about this
//! process, a child that stages a file through the production functions and is
//! still running, and the same child dead without warning, with a real pid in
//! every case.
//!
//! ## Owns
//! - The real probe's answers about this process: alive now, and gone when the
//!   recorded start time is not this process's, which is a recycled pid's shape
//! - The child half, which stages a member, records it in this process's own
//!   journal, and then either returns or dies without returning
//! - The parent half, which runs the real `sweep_staging_journals` against a
//!   directory a real process left behind
//! - The subprocess guard that keeps a failed test from leaving a process
//!   running
//!
//! ## Does not own
//! - The sweep's decisions with the liveness answer injected (see
//!   `staging_journal_tests`)
//! - The journal's own bytes, the name rules, and the removal rule (see
//!   `staging_journal` and `staging_journal_name`)
//!
//! ## Boundary
//!
//! Changes to how the sweep reads the host's process table should normally be
//! proved here as well as in the injected cases. Changes to what a journal
//! records should not require changes here.

use super::*;
use crate::fixtures::{Scratch, dir_entries};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// The directory the child works in. Absent in a normal run, which is what
/// leaves the helper test below with nothing to do.
const CHILD_ROOT: &str = "CLIO_STAGING_JOURNAL_CHILD";

/// Whether the child returns when it is released or dies where it stands.
const CHILD_MODE: &str = "CLIO_STAGING_JOURNAL_CHILD_MODE";

/// The marker the child writes once its staged file and its journal entry are
/// both on disk. It is the synchronization between the two halves: the parent
/// does not sweep, release, or wait before this file exists.
const READY: &str = "ready";

/// The marker the parent writes to let an alive child return.
const RELEASE: &str = "release";

/// The name the harness gives the child helper, which is this module's path
/// without the crate name: libtest lists a unit test under its module path
/// alone, and `--exact` needs that name.
const HELPER: &str = "a_journal_owner_child_stages_a_file";

/// How long a marker or an exit may take. This is a safety net, not the
/// synchronization: the markers are, and this only turns a child that never
/// signals into a failure instead of a hang. It is deliberately far larger than
/// the milliseconds a child actually needs.
const DEADLINE: Duration = Duration::from_secs(120);

/// How long to wait between two looks at a marker.
const POLL: Duration = Duration::from_millis(10);

/// A child that is killed and reaped when the test ends, so a panic between
/// spawning it and waiting for it cannot leave a process running.
struct ChildGuard(Option<Child>);

impl ChildGuard {
    /// Start this very test binary again, running only the helper test, with
    /// `root` and `mode` in the child's environment. This process's own
    /// environment is never touched.
    fn spawn(root: &Path, mode: &str) -> Self {
        let child = Command::new(std::env::current_exe().expect("this test binary"))
            .arg("--exact")
            .arg(helper_test_name())
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(CHILD_ROOT, root)
            .env(CHILD_MODE, mode)
            .spawn()
            .expect("spawn the child process");
        Self(Some(child))
    }

    /// Reap the child, or `None` if it is still running when the deadline
    /// passes. Reaping matters: a process that has exited but has not been
    /// waited for still has a `/proc/<pid>/stat`, so a sweep run against a
    /// zombie would prove nothing.
    fn wait(&mut self) -> Option<ExitStatus> {
        let child = self.0.as_mut()?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {}
                Err(_) => return None,
            }
            if start.elapsed() >= DEADLINE {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The full name the harness knows this module's helper test by.
fn helper_test_name() -> String {
    let module = module_path!();
    // Split on the FIRST `::`, which is the crate name, and keep the rest.
    let without_crate = module.split_once("::").map_or(module, |(_, rest)| rest);
    format!("{without_crate}::{HELPER}")
}

/// The marker file `name` in the signals directory under `root`.
fn signal(root: &Path, name: &str) -> PathBuf {
    root.join("signals").join(name)
}

/// Wait until `marker` exists, or fail the test with the marker in the message.
fn wait_for(marker: &Path) {
    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < DEADLINE,
            "{} never appeared within {DEADLINE:?}",
            marker.display()
        );
        std::thread::sleep(POLL);
    }
}

/// The name of a path inside its own directory, as a string.
fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// A staged member's name in the shape `staging::reserve_path` hands out. Spelled
/// out here rather than shared with the sibling test module, because a staged
/// member's *name* is what the probe cases are about and the sweep cases below
/// never need one.
fn staged_name(tag: &str) -> String {
    format!("{STAGING_PREFIX}{tag}-4242-1700000000000000000-0{STAGED_SUFFIX}")
}

/// An entry shaped like a live owner: this process's own pid, and this
/// process's own start time where the host reports one.
fn entry_for_this_process(name: &str) -> JournalEntry {
    JournalEntry {
        name: name.to_owned(),
        pid: std::process::id(),
        start_ticks: self_start_ticks(),
    }
}

/// The probe must call a running process alive. This is the one place the real
/// `OwnerProbe` is asked about a process it cannot possibly be wrong about: its
/// own, with its own start time read from its own `/proc`.
#[test]
fn the_real_probe_reports_this_process_as_alive() {
    let entry = entry_for_this_process(&staged_name("restore-payload"));

    assert_eq!(ProcfsProbe.state(&entry), OwnerState::Alive);
}

/// A pid whose start time is not the one the entry recorded belongs to a
/// different process, so the one that recorded the entry is gone. This is the
/// recycled-pid case against the real probe rather than an injected answer: the
/// entry is this process's own pid with its start time off by one tick, which is
/// the only shape a sweep can actually be handed.
#[test]
fn the_real_probe_reports_a_recycled_pid_as_gone() {
    let entry = JournalEntry {
        start_ticks: self_start_ticks().map(|ticks| ticks.wrapping_add(1)),
        ..entry_for_this_process(&staged_name("restore-payload"))
    };

    assert_eq!(
        ProcfsProbe.state(&entry),
        OwnerState::Gone,
        "a pid with another start time is a recycled pid"
    );
}

/// The one staged member the child left in `dir`.
fn the_only_staged_file(dir: &Path) -> PathBuf {
    let found = files_named_in(dir, is_staged_name);
    assert_eq!(
        found.len(),
        1,
        "the child staged exactly one file: {found:?}"
    );
    found.into_iter().next().expect("one staged file")
}

/// The one journal the child left in `dir`.
fn the_only_journal(dir: &Path) -> PathBuf {
    let found = files_named_in(dir, is_journal_name);
    assert_eq!(
        found.len(),
        1,
        "the child armed exactly one journal: {found:?}"
    );
    found.into_iter().next().expect("one journal")
}

/// The files in `dir` whose name `wanted` accepts.
fn files_named_in(dir: &Path, wanted: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read the staging directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && wanted(&file_name(path)))
        .collect()
}

/// The child half of both cases: stage a member, record it in this process's
/// own journal, and then either return or die.
///
/// A normal run has no child root in its environment, so it returns at once and
/// this test passes without doing anything. The parent runs this very test in a
/// second copy of this binary, which is the only way to get a real pid that
/// really goes away.
#[test]
fn a_journal_owner_child_stages_a_file() {
    let Ok(root) = std::env::var(CHILD_ROOT) else {
        return;
    };
    let root = PathBuf::from(root);
    let mode = std::env::var(CHILD_MODE).unwrap_or_default();
    // Only a mode the parent actually asks for is a child request, so a stray
    // variable in someone's environment cannot turn a normal run into one.
    if !matches!(mode.as_str(), "alive" | "dead") {
        return;
    }
    let dir = root.join("staging");
    assert!(
        crate::staging::ensure_backup_dir(&dir).expect("create the staging directory"),
        "the staging directory is new"
    );
    std::fs::create_dir_all(root.join("signals")).expect("create the signals directory");

    // `None` first, so the journal is armed by the same call a restore makes
    // rather than by hand.
    let mut journal: Option<StagingJournal> = None;
    let staged = crate::staging::stage_bytes(b"staged by the child", &dir, "restore-payload")
        .expect("stage a member in the child");
    record_staged(&mut journal, &dir, staged.path()).expect("record the staged path");
    // Written last, so the parent never looks at a half-built directory.
    std::fs::write(signal(&root, READY), b"").expect("signal that the child is ready");

    if mode == "alive" {
        wait_for(&signal(&root, RELEASE));
        // A normal return: both drops run, which is what the alive case looks
        // for once it has swept.
        drop(staged);
        drop(journal);
        return;
    }
    // The same class as the signal that kills a restore: the process never
    // returns, unwinds nothing, and runs no `Drop`, so the staged file and the
    // journal stay on disk. `abort` is used because the workspace denies
    // `unsafe`, so there is no `kill(2)` call to make.
    std::process::abort();
}

#[test]
fn a_live_owner_keeps_its_staged_file_and_its_journal() {
    let scratch = Scratch::new("journal-live-owner");
    let dir = scratch.path().join("staging");
    let mut child = ChildGuard::spawn(scratch.path(), "alive");

    wait_for(&signal(scratch.path(), READY));
    let staged = the_only_staged_file(&dir);
    let journal = the_only_journal(&dir);

    let report = sweep_staging_journals(&dir);

    assert_eq!(
        report.staged_files_removed, 0,
        "a live owner keeps its staged file"
    );
    assert!(staged.exists(), "the staged file is still on disk");
    assert!(journal.exists(), "and the journal that names it");

    std::fs::write(signal(scratch.path(), RELEASE), b"").expect("release the child");
    let status = child.wait().expect("the child exited within the deadline");
    assert!(status.success(), "the child returned normally: {status:?}");
    assert_eq!(
        dir_entries(&dir),
        Vec::<String>::new(),
        "and a normal return leaves nothing behind"
    );
}

#[test]
fn a_dead_owner_loses_its_staged_file_and_its_journal() {
    let scratch = Scratch::new("journal-dead-owner");
    let dir = scratch.path().join("staging");
    let mut child = ChildGuard::spawn(scratch.path(), "dead");

    wait_for(&signal(scratch.path(), READY));
    let status = child.wait().expect("the child exited within the deadline");
    assert!(!status.success(), "the child really died: {status:?}");
    let staged = the_only_staged_file(&dir);
    let journal = the_only_journal(&dir);

    let report = sweep_staging_journals(&dir);

    assert_eq!(
        report.staged_files_removed, 1,
        "the dead owner's staged file is removed"
    );
    assert!(!staged.exists(), "and it is gone from the directory");
    assert!(!journal.exists(), "and the journal that named it is gone");
}
