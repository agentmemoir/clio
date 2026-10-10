// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the staging journal: what a journal
//! records and does not, that only a proven-gone owner is swept, that a file
//! that is not a staged member's own is never removed, and that the sweep reads
//! the host's process table the way it claims to.
//!
//! ## Owns
//! - The sweep cases, with the liveness answer injected so "the owner is alive"
//!   and "the owner is gone" never depend on a real process
//! - The journal's own bytes and mode, and the journal's lifetime across an
//!   extraction
//!
//! ## Does not own
//! - The staging assertions (see `staging_tests`)
//! - The extraction refusals (see `extract_tests` and `extract_staging_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the journal and the sweep.

use super::*;
use crate::create::create_archive;
use crate::extract::extract_members;
use crate::fixtures::{Scratch, dir_entries, request, staged_payload};
use crate::inventory::read_manifest;
use crate::manifest::BackupManifest;
use std::fmt::Write as _;
use std::path::PathBuf;

/// A probe whose answer is fixed, so the sweep's two answers are decided here
/// and never depend on a real pid, a real process exiting, or the host's procfs.
struct FixedProbe {
    state: OwnerState,
}

impl OwnerProbe for FixedProbe {
    fn state(&self, _entry: &JournalEntry) -> OwnerState {
        self.state
    }
}

/// A probe that has swept one journal and is the wrong answer for the test.
fn gone() -> FixedProbe {
    FixedProbe {
        state: OwnerState::Gone,
    }
}

/// A staged member's name in the shape `staging::reserve_path` hands out.
fn staged_name(tag: &str) -> String {
    format!("{STAGING_PREFIX}{tag}-4242-1700000000000000000-0{STAGED_SUFFIX}")
}

/// Write a staged-looking file in `dir`; its path and its name.
fn write_staged_file(dir: &Path, tag: &str) -> (PathBuf, String) {
    let name = staged_name(tag);
    let path = dir.join(&name);
    std::fs::write(&path, b"staged member bytes").expect("write the staged file");
    (path, name)
}

/// A journal written by hand for a pid the test names, so a sweep's decision is
/// about the recorded owner and not about a process that has to be made to go
/// away first.
fn written_journal(dir: &Path, pid: u32, names: &[&str]) -> PathBuf {
    journal_named(
        dir,
        &format!("{STAGING_PREFIX}{JOURNAL_TAG}-{pid}-1700000000000000000-0{JOURNAL_SUFFIX}"),
        pid,
        names,
    )
}

/// The same, under a name the caller chooses, so a sweep can meet a file that
/// wears the family's name without being a journal this sweep ever wrote.
fn journal_named(dir: &Path, file_name: &str, pid: u32, names: &[&str]) -> PathBuf {
    let path = dir.join(file_name);
    let mut raw = String::new();
    for name in names {
        let entry = JournalEntry {
            name: (*name).to_owned(),
            pid,
            start_ticks: Some(4242),
        };
        raw.push_str(&serde_json::to_string(&entry).expect("serialize the entry"));
        raw.push('\n');
    }
    std::fs::write(&path, raw).expect("write the journal");
    path
}

/// The journal files in `dir`, sorted.
fn journal_names(dir: &Path) -> Vec<String> {
    dir_entries(dir)
        .into_iter()
        .filter(|name| is_journal_name(name))
        .collect()
}

/// A real archive and its manifest, for the extraction cases.
fn archive_and_manifest(scratch: &Scratch) -> (PathBuf, BackupManifest) {
    let payload = staged_payload(scratch.path(), 61);
    let report = create_archive(&request(scratch.path(), payload)).expect("create the archive");
    let manifest = read_manifest(&report.path).expect("read the manifest back");
    (report.path, manifest)
}

/// A fresh owner-only directory to extract into.
fn restore_dir(scratch: &Scratch) -> PathBuf {
    let dir = scratch.join("restored");
    assert!(
        crate::staging::ensure_backup_dir(&dir).expect("create the staging directory"),
        "the staging directory is new"
    );
    dir
}

#[test]
fn a_staged_file_whose_owner_is_alive_is_not_swept() {
    let scratch = Scratch::new("journal-owner-alive");
    let (staged, name) = write_staged_file(scratch.path(), "restore-payload");
    written_journal(scratch.path(), 4242, &[name.as_str()]);

    let report = sweep_with(
        scratch.path(),
        &FixedProbe {
            state: OwnerState::Alive,
        },
    );

    assert!(staged.exists(), "a live owner's staged file is not removed");
    assert_eq!(report.staged_files_removed, 0, "nothing is removed");
    assert_eq!(report.journals_left_alone, 1, "the journal is left too");
}

#[test]
fn a_staged_file_whose_owner_is_gone_is_swept() {
    let scratch = Scratch::new("journal-owner-gone");
    let (staged, name) = write_staged_file(scratch.path(), "restore-payload");
    written_journal(scratch.path(), 4242, &[name.as_str()]);

    sweep_with(scratch.path(), &gone());

    assert!(
        !staged.exists(),
        "a staged file whose owner is gone is removed"
    );
}

#[test]
fn a_swept_journal_is_removed() {
    let scratch = Scratch::new("journal-removed");
    let (_, name) = write_staged_file(scratch.path(), "restore-payload");
    let journal = written_journal(scratch.path(), 4242, &[name.as_str()]);

    sweep_with(scratch.path(), &gone());

    assert!(!journal.exists(), "the journal has nothing left to say");
    assert_eq!(journal_names(scratch.path()), Vec::<String>::new());
}

#[test]
fn a_journal_records_the_path_and_the_pid_and_nothing_of_the_member() {
    let scratch = Scratch::new("journal-bytes");
    let mut journal = StagingJournal::create(scratch.path()).expect("arm a journal");
    let name = staged_name("restore-key");
    let staged = scratch.join(&name);
    std::fs::write(&staged, b"key-material-bytes").expect("write the staged file");
    journal.record(&staged).expect("record the staged path");

    let raw = std::fs::read_to_string(journal.path()).expect("read the journal");

    assert!(raw.contains(&name), "the staged path is recorded: {raw}");
    assert!(
        raw.contains(&std::process::id().to_string()),
        "the owning pid is recorded: {raw}"
    );
    assert!(!raw.contains("key-material-bytes"), "no key bytes: {raw}");
    assert!(
        !raw.contains(crate::KEY_MEMBER),
        "no archive member name: {raw}"
    );
}

#[cfg(unix)]
#[test]
fn a_journal_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("journal-mode");

    let journal = StagingJournal::create(scratch.path()).expect("arm a journal");

    let mode = std::fs::metadata(journal.path())
        .expect("read the journal's metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "only the owner may read the journal");
}

#[test]
fn a_file_that_is_not_staging_named_is_never_swept() {
    let scratch = Scratch::new("journal-not-staged");
    let archive = scratch.join("clio-20260927T000000Z-abcd1234.cliobak.tar.gz");
    std::fs::write(&archive, b"the published archive").expect("write the archive");
    let name = archive
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    written_journal(scratch.path(), 4242, &[name.as_str()]);

    sweep_with(scratch.path(), &gone());

    assert!(
        archive.exists(),
        "a journal entry cannot have an archive removed through it"
    );
}

#[test]
fn a_journal_entry_that_names_a_path_out_of_the_directory_sweeps_nothing() {
    let scratch = Scratch::new("journal-traversal");
    let dir = scratch.join("swept");
    std::fs::create_dir(&dir).expect("the directory being swept");
    // The hop has to exist for the kernel to resolve a path through it, and the
    // name climbs out of the swept directory with one `..` per level: joined
    // against `swept` and resolved by the kernel, this names
    // `<scratch>/victim-5-6-7.tmp`, which is OUTSIDE the directory being swept.
    // A name that stayed inside would pass this test without the fix.
    std::fs::create_dir(dir.join("clio-snapshot-x")).expect("the hop out of the directory");
    let victim = scratch.join("victim-5-6-7.tmp");
    std::fs::write(&victim, b"a file outside the swept directory").expect("write the victim");
    let entry = "clio-snapshot-x/../../victim-5-6-7.tmp";
    written_journal(&dir, 4242, &[entry]);

    let report = sweep_with(&dir, &gone());

    assert!(victim.exists(), "a name that is a path removes nothing");
    assert_eq!(report.staged_files_removed, 0, "nothing is removed");
    // The name is a path, not a staged member's own name, whatever its fields
    // parse as.
    assert!(
        !is_staged_name(entry),
        "the name rule rejects a name that is not one leaf: {entry}"
    );
}

#[test]
fn a_journal_that_was_never_written_is_neither_read_nor_removed() {
    for foreign in [
        "clio-snapshot-anything.journal",
        "clio-snapshot-restore-journal-4242-not-a-number-0.journal",
    ] {
        let scratch = Scratch::new("journal-foreign");
        let (staged, name) = write_staged_file(scratch.path(), "restore-payload");
        let foreign = journal_named(scratch.path(), foreign, 4242, &[name.as_str()]);

        let report = sweep_with(scratch.path(), &gone());

        assert_eq!(report.journals_examined, 0, "{foreign:?} is not a journal");
        assert!(staged.exists(), "{foreign:?} does not get its entry swept");
        assert!(foreign.exists(), "{foreign:?} is not removed either");
    }
}

#[test]
fn a_journal_is_kept_when_a_staged_file_it_names_cannot_be_removed() {
    let scratch = Scratch::new("journal-kept");
    let (staged, name) = write_staged_file(scratch.path(), "restore-payload");
    // `remove_file` on a directory fails on any host, with no root and no
    // permission trick, so this is a removal that fails for a reason the sweep
    // must not swallow.
    let blocked_name = staged_name("restore-table");
    let blocked = scratch.join(&blocked_name);
    std::fs::create_dir(&blocked).expect("the directory in the way");
    let journal = written_journal(
        scratch.path(),
        4242,
        &[name.as_str(), blocked_name.as_str()],
    );

    let report = sweep_with(scratch.path(), &gone());

    assert!(!staged.exists(), "the staged file that could be removed is");
    assert!(blocked.exists(), "the one that could not is still there");
    assert!(
        journal.exists(),
        "so the journal is kept for the next sweep"
    );
    assert_eq!(report.journals_left_alone, 1, "the kept journal is counted");
    assert_eq!(report.staged_files_removed, 1, "only the file that went");
}

#[test]
fn liveness_that_cannot_be_determined_sweeps_nothing() {
    let scratch = Scratch::new("journal-unknown");
    let (staged, name) = write_staged_file(scratch.path(), "restore-key");
    written_journal(scratch.path(), 4242, &[name.as_str()]);

    let report = sweep_with(
        scratch.path(),
        &FixedProbe {
            state: OwnerState::Undetermined,
        },
    );

    assert!(
        staged.exists(),
        "a host that cannot say leaves the staged file alone"
    );
    assert_eq!(report.journals_left_alone, 1, "the journal is left too");
}

#[test]
fn a_second_sweep_over_the_same_directory_removes_nothing_more() {
    let scratch = Scratch::new("journal-sweep-twice");
    let (staged, name) = write_staged_file(scratch.path(), "restore-payload");
    written_journal(scratch.path(), 4242, &[name.as_str()]);
    let first = sweep_with(scratch.path(), &gone());
    assert!(!staged.exists(), "the first sweep removed it");

    let second = sweep_with(scratch.path(), &gone());

    assert_eq!(first.staged_files_removed, 1, "the first sweep removed it");
    assert_eq!(
        second,
        JournalSweepReport::default(),
        "a directory with no journal left in it has nothing to sweep"
    );
}

#[test]
fn a_torn_last_journal_line_is_skipped_and_the_other_entries_are_honoured() {
    let scratch = Scratch::new("journal-torn");
    let (whole, whole_name) = write_staged_file(scratch.path(), "restore-payload");
    let (torn, torn_name) = write_staged_file(scratch.path(), "restore-table");
    let journal = written_journal(scratch.path(), 4242, &[]);
    let entry = JournalEntry {
        name: whole_name,
        pid: 4242,
        start_ticks: Some(4242),
    };
    let mut raw = format!(
        "{}\n",
        serde_json::to_string(&entry).expect("serialize the entry")
    );
    let _ = write!(
        raw,
        "{{\"name\":{},\"pid\":424",
        serde_json::to_string(&torn_name).expect("serialize the name")
    );
    std::fs::write(&journal, raw).expect("write the torn journal");

    sweep_with(scratch.path(), &gone());

    assert!(!whole.exists(), "the line that parsed is still honoured");
    assert!(
        torn.exists(),
        "the torn line names nothing, so nothing goes"
    );
}

#[test]
fn a_journal_recording_this_process_is_never_swept() {
    let scratch = Scratch::new("journal-own-pid");
    let (staged, name) = write_staged_file(scratch.path(), "restore-payload");
    written_journal(scratch.path(), std::process::id(), &[name.as_str()]);

    sweep_with(scratch.path(), &gone());

    assert!(
        staged.exists(),
        "this process's own staged file is never swept, whatever the probe says"
    );
}

/// The start time one `/proc/<pid>/stat` line ends with. One constant for the
/// value, so the separators clippy asks for never end up inside the text the
/// parser is given.
const START_TICKS: u64 = 8_675_309;

#[test]
fn the_start_time_is_read_past_a_comm_with_spaces_and_a_close_paren() {
    // Fields 3 to 21 are noise; field 22 is the start time the sweep reads, and
    // the `comm` field carries a space and a `)` of its own.
    let noise = (3..=21)
        .map(|field| field.to_string())
        .collect::<Vec<String>>()
        .join(" ");
    let stat = format!("4242 (pg_dump) worker) {noise} {START_TICKS}");

    assert_eq!(start_ticks(&stat), Some(START_TICKS));
    assert_eq!(
        start_ticks("4242 pg_dump 8675309"),
        None,
        "no closing parenthesis means no field 22 to read"
    );
}

#[test]
fn a_normal_return_leaves_no_journal_and_no_staged_file_behind() {
    let scratch = Scratch::new("journal-after-return");
    let (archive, manifest) = archive_and_manifest(&scratch);
    let dir = restore_dir(&scratch);

    let extracted = extract_members(&archive, &manifest, &dir).expect("extract");
    drop(extracted);

    assert_eq!(dir_entries(&dir), Vec::<String>::new());
}

#[test]
fn the_extracted_archive_owns_its_journal() {
    let scratch = Scratch::new("journal-owned");
    let (archive, manifest) = archive_and_manifest(&scratch);
    let dir = restore_dir(&scratch);

    let extracted = extract_members(&archive, &manifest, &dir).expect("extract");
    let while_alive = journal_names(&dir);
    drop(extracted);

    assert_eq!(while_alive.len(), 1, "one journal while the value lives");
    assert_eq!(journal_names(&dir), Vec::<String>::new(), "none after");
}
