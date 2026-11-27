// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the file-system mechanics on one held
//! directory: the entry identity the confirming stat compares, a root that
//! resolves but cannot be opened, an inspection that cannot read the name, and an
//! unlink the directory refuses.
//!
//! ## Owns
//! - The `same_entry` assertions: one entry compared with itself, two different
//!   files, and a hard link to one inode
//! - The `hold` refusals a real path can produce: a regular file in the place of
//!   the backup directory
//! - The `remove` refusals a real name can produce: a name too long to inspect,
//!   and an unlink a read-only directory will not make
//! - The `refuse_unconfirmed` refusal a platform without a descriptor and an
//!   inode identity reports for every confirmed candidate
//!
//! ## Does not own
//! - The removal rules themselves (see `prune_delete`)
//! - The containment cases (see `prune_delete::containment_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::super::delete;
use super::super::tests::{candidate, detail_of, file_len, plan_with, states_of};
use super::*;
use crate::SNAPSHOT_EXTENSION;
use crate::fixtures::{Scratch, dir_entries, write_is_refused};
use rustix::fs::{AtFlags, statat};

/// The three names the fixture writes into one held directory.
const FIRST: &str = "first.cliobak.tar.gz";
const SECOND: &str = "second.cliobak.tar.gz";
const ALIAS: &str = "alias.cliobak.tar.gz";

/// Device and inode together are one entry's identity.
///
/// The same file stat'd twice through the held descriptor compares equal, so a
/// run that inspected and confirmed one name removes it. Two different files in
/// one directory compare unequal, which is what turns a swap in the gap into a
/// refusal instead of a silent removal of something never inspected. A hard link
/// to the same inode under a second name compares equal, because a second name
/// for one file is still that one file.
#[test]
fn the_identity_confirm_matches_one_entry_and_separates_two_files() {
    let scratch = Scratch::new("prune-identity");
    std::fs::write(scratch.join(FIRST), b"one").expect("write the first file");
    std::fs::write(scratch.join(SECOND), b"a different length").expect("write the second");
    std::fs::hard_link(scratch.join(FIRST), scratch.join(ALIAS))
        .expect("a hard link to the first file");
    let held = hold(scratch.path()).expect("hold the directory open");
    let stat = |name: &str| {
        statat(&held.dir, name, AtFlags::SYMLINK_NOFOLLOW)
            .expect("stat a bare name in the held dir")
    };

    let first = stat(FIRST);
    let again = stat(FIRST);
    let second = stat(SECOND);
    let alias = stat(ALIAS);

    assert!(
        same_entry(&first, &again),
        "the same file stat'd twice is one entry"
    );
    assert!(
        !same_entry(&first, &second),
        "two different files are two entries"
    );
    assert!(
        same_entry(&first, &alias),
        "a hard link names the same inode"
    );
    assert_eq!(
        first.st_size, alias.st_size,
        "the size credited comes from the inspection"
    );
    assert_ne!(
        first.st_size, second.st_size,
        "the fixture files differ in size on purpose"
    );
}

/// A backup directory that resolves but cannot be opened as a directory fails
/// every candidate, names the reason, and removes nothing.
///
/// The path handed in is a regular file, so `canonicalize` succeeds and the open
/// that demands a directory is what refuses. That is the second refusal `hold`
/// has and the one a path-value check cannot see: the path exists and resolves,
/// so only asking the file system for a directory tells the run to stop. Every
/// candidate fails with the same reason, and none falls back to a path-based
/// removal, because a run that cannot hold the directory knows nothing about
/// where the files really are.
#[test]
fn a_backup_directory_that_is_a_regular_file_fails_every_candidate() {
    let scratch = Scratch::new("prune-file-root");
    let elsewhere = Scratch::new("prune-file-root-kept");
    let orphan = elsewhere.path().join(format!("orphan{SNAPSHOT_EXTENSION}"));
    std::fs::write(&orphan, b"a file this run must not touch")
        .expect("write a file to leave alone");
    let orphan_bytes = file_len(&orphan);
    let not_a_directory = scratch.join("not-a-directory");
    std::fs::write(&not_a_directory, b"a regular file, not a backup directory")
        .expect("write a regular file where the backup directory was expected");
    let under_it = not_a_directory.join(format!("under-it{SNAPSHOT_EXTENSION}"));
    let candidates = vec![
        candidate("under-it", &under_it, 0),
        candidate("orphan", &orphan, orphan_bytes),
    ];
    let plan = plan_with(candidates, 0);

    let report = delete(&not_a_directory, &plan);

    assert_eq!(
        states_of(&report),
        vec![
            ("under-it", CandidateState::Failed),
            ("orphan", CandidateState::Failed),
        ],
        "a root that is not a directory fails every candidate"
    );
    for outcome in &report.candidates {
        let detail = detail_of(outcome);
        assert!(
            detail.contains("could not be opened"),
            "{outcome:?} must say the directory was not opened"
        );
        assert!(
            detail.contains(&not_a_directory.display().to_string()),
            "{outcome:?} must name the directory it could not open"
        );
        assert!(
            !detail.contains("could not be resolved"),
            "{outcome:?} resolved, so it must not claim otherwise"
        );
    }
    assert!(report.has_failures(), "the caller sees the failure");
    assert_eq!(
        report.candidates[0].state.label(),
        "failed",
        "so a report says so in one word"
    );
    assert_eq!(report.reclaimed_bytes, 0, "nothing was unlinked");
    assert_eq!(
        report.remaining_bytes, plan.listed_bytes,
        "every byte is still there"
    );
    assert!(
        not_a_directory.is_file(),
        "the file the caller named is untouched"
    );
    assert!(orphan.is_file(), "and so is the archive it was told about");
    assert_eq!(file_len(&orphan), orphan_bytes, "which keeps its bytes");
}

/// A name the file system will not stat is reported as a failure with its
/// reason, and nothing is removed.
///
/// The bare name is longer than the file system's per-component limit, so the
/// inspection itself is refused and the run never gets as far as a removal. That
/// is the arm the inspection needs: a stat that fails for a reason other than
/// "already gone" cannot be read as a skip, because a name nobody could read is
/// not a name anybody proved absent.
#[test]
fn a_candidate_name_too_long_to_inspect_is_reported_as_a_failure() {
    let scratch = Scratch::new("prune-long-name");
    let too_long = format!("{}{SNAPSHOT_EXTENSION}", "a".repeat(300));
    let unreachable = scratch.join(&too_long);
    let plan = plan_with(vec![candidate("too-long", &unreachable, 0)], 0);

    let report = delete(scratch.path(), &plan);

    let outcome = &report.candidates[0];
    assert_eq!(outcome.state, CandidateState::Failed);
    assert_eq!(outcome.state.label(), "failed", "so a report says so");
    assert!(
        detail_of(outcome).contains("cannot read the archive"),
        "{outcome:?} must say the name could not be read"
    );
    assert!(
        detail_of(outcome).contains("too long"),
        "{outcome:?} must carry the reason the file system gave"
    );
    assert_eq!(outcome.reclaimed_bytes, 0, "nothing was credited");
    assert_eq!(report.reclaimed_bytes, 0, "and the run freed nothing");
    assert!(report.has_failures(), "the caller sees the failure");
    assert_eq!(
        report.remaining_bytes, plan.listed_bytes,
        "every byte is still there"
    );
    assert_eq!(
        dir_entries(scratch.path()),
        Vec::<String>::new(),
        "nothing was written or removed"
    );
}

/// An unlink the directory refuses is reported, and the archive keeps its bytes.
///
/// The archive is inspected and confirmed like any other, so the refusal comes
/// from the unlink itself: the directory's own mode stops the removal. That is
/// the last arm of the sequence and the only one that proves a removal failure is
/// reported as a failure, with the reason, and never credited as a reclaim.
///
/// The refusal is only observable to a process the mode bits restrict. Root, and
/// any process holding `CAP_DAC_OVERRIDE`, unlinks from the directory anyway, so
/// the test asks the process's own access which case this is and asserts what
/// that case allows.
#[cfg(unix)]
#[test]
fn an_unlink_the_directory_refuses_is_reported_and_the_archive_stays() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("prune-unlink-refused");
    let archive = scratch.path().join(format!("stuck{SNAPSHOT_EXTENSION}"));
    std::fs::write(&archive, b"an archive the directory will not give up")
        .expect("write the archive");
    let bytes = file_len(&archive);
    let plan = plan_with(vec![candidate("stuck", &archive, bytes)], 0);
    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o500))
        .expect("make the directory read-only");
    let refused = write_is_refused(scratch.path());

    let report = delete(scratch.path(), &plan);

    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700))
        .expect("restore the mode so the scratch guard can remove the directory");

    let outcome = &report.candidates[0];
    if refused {
        assert_eq!(
            outcome.state,
            CandidateState::Failed,
            "a refused unlink is a failure, not a removal: {outcome:?}"
        );
        assert!(
            detail_of(outcome).contains("removing"),
            "{outcome:?} must name what it could not remove"
        );
        assert!(
            !detail_of(outcome).contains("another process"),
            "{outcome:?} must not call a refusal an absence"
        );
        assert_eq!(outcome.reclaimed_bytes, 0, "nothing was credited");
        assert_eq!(report.reclaimed_bytes, 0, "and the run freed nothing");
        assert!(report.has_failures(), "the caller sees the failure");
        assert_eq!(
            report.remaining_bytes, plan.listed_bytes,
            "every byte is still there"
        );
        assert!(archive.is_file(), "the archive is still there");
        assert_eq!(file_len(&archive), bytes, "and it keeps every byte");
    } else {
        // This process bypasses the mode bits, so the directory is not the
        // refusal under test: the archive goes, and the run credits its bytes.
        assert_eq!(outcome.state, CandidateState::Deleted);
        assert_eq!(report.reclaimed_bytes, bytes, "the bytes it removed");
        assert!(!archive.exists(), "a DAC-bypassing process unlinked it");
    }
}

/// An entry replaced between the inspection and the unlink is refused, and the
/// replacement keeps its bytes.
///
/// The hook acts in the one moment the confirm exists to close: after the
/// inspection and before the second stat. The second stat sees a different inode,
/// so the run refuses instead of unlinking a file it never inspected, and credits
/// nothing. The replacement itself stays on disk, because the run removed
/// nothing at all.
#[cfg(unix)]
#[test]
fn an_entry_replaced_between_the_inspection_and_the_unlink_is_refused() {
    let scratch = Scratch::new("prune-swap-gap");
    let archive = scratch.path().join(format!("swapped{SNAPSHOT_EXTENSION}"));
    std::fs::write(&archive, b"the archive the run inspected")
        .expect("write the archive the plan names");
    std::fs::write(
        scratch.join("replacement.bin"),
        b"a different file entirely",
    )
    .expect("write the file that takes its place");
    let replacement_bytes = file_len(&scratch.join("replacement.bin"));
    let plan = plan_with(vec![candidate("swapped", &archive, file_len(&archive))], 0);
    let _hook = gap::set(|dir, name| {
        rustix::fs::renameat(dir, "replacement.bin", dir, name)
            .expect("swap the inspected entry for another file in the gap");
    });

    let report = delete(scratch.path(), &plan);

    let outcome = &report.candidates[0];
    assert_eq!(
        outcome.state,
        CandidateState::Failed,
        "a swapped entry is refused, not removed: {outcome:?}"
    );
    assert!(
        detail_of(outcome).contains("changed after it was inspected"),
        "{outcome:?} must say the entry changed"
    );
    assert!(
        !detail_of(outcome).contains("another process"),
        "{outcome:?} must not call a swap an absence"
    );
    assert_eq!(outcome.reclaimed_bytes, 0, "nothing was credited");
    assert_eq!(report.reclaimed_bytes, 0, "and the run freed nothing");
    assert!(report.has_failures(), "the caller sees the failure");
    assert_eq!(
        report.remaining_bytes, plan.listed_bytes,
        "every byte is still there"
    );
    assert!(archive.is_file(), "the replacement is still there");
    assert_eq!(
        file_len(&archive),
        replacement_bytes,
        "and it keeps every byte of its own"
    );
}

/// An entry removed between the inspection and the unlink is reported as already
/// gone, because that is what the code does.
///
/// The hook removes the entry in the same gap. The confirming stat then finds no
/// entry at all, and the run reports the state it reports for an absent name: the
/// desired end state holds, so it is not a failure and nothing is credited. The
/// state is asserted as the code produces it, not changed to suit the test.
#[cfg(unix)]
#[test]
fn an_entry_removed_between_the_inspection_and_the_unlink_reports_already_gone() {
    let scratch = Scratch::new("prune-removed-gap");
    let archive = scratch.path().join(format!("vanished{SNAPSHOT_EXTENSION}"));
    std::fs::write(&archive, b"the archive the run inspected").expect("write the archive");
    let bytes = file_len(&archive);
    let plan = plan_with(vec![candidate("vanished", &archive, bytes)], 0);
    let _hook = gap::set(|dir, name| {
        rustix::fs::unlinkat(dir, name, rustix::fs::AtFlags::empty())
            .expect("remove the inspected entry in the gap");
    });

    let report = delete(scratch.path(), &plan);

    let outcome = &report.candidates[0];
    assert_eq!(
        outcome.state,
        CandidateState::AlreadyGone,
        "the confirm finds no entry, which is the already-gone state: {outcome:?}"
    );
    assert!(
        detail_of(outcome).contains("another process"),
        "{outcome:?} must say why nothing was removed"
    );
    assert!(!outcome.state.is_failure(), "the end state already holds");
    assert_eq!(outcome.reclaimed_bytes, 0, "nothing was credited");
    assert_eq!(report.reclaimed_bytes, 0, "and the run freed nothing");
    assert!(!report.has_failures(), "an absent entry is not a failure");
    assert!(!archive.exists(), "the entry really is gone");
}

/// A removal this platform cannot confirm is refused, and the refusal is a
/// failure that credits nothing.
///
/// The non-unix `remove` body is compiled out on this host, so the test calls
/// `refuse_unconfirmed` directly: that is the call the non-unix confirmed path
/// makes once the entry is a plain file and removal was asked for. Only a
/// cross-target check of the non-unix dispatch proves that this call compiles and
/// is reached there; no run on this host can execute that body, so this test
/// proves the refusal itself and not the dispatch. The outcome is decided from the
/// candidate alone, so no file is written and no clock is read.
#[test]
fn a_removal_this_platform_cannot_confirm_is_refused() {
    let path = PathBuf::from(format!("unconfirmed{SNAPSHOT_EXTENSION}"));
    let archive = candidate("unconfirmed", &path, 4096);
    let pending = CandidateOutcome {
        id: archive.id.clone(),
        path: archive.path.clone(),
        created_at: archive.created_at.clone(),
        backend: archive.backend.clone(),
        slug: archive.slug.clone(),
        listed_bytes: archive.listed_bytes,
        reclaimed_bytes: 0,
        state: CandidateState::Pending,
        detail: None,
    };

    let refused = refuse_unconfirmed(&archive, pending);

    assert_eq!(
        refused.state,
        CandidateState::Failed,
        "a removal this platform cannot confirm is not a removal"
    );
    assert_eq!(refused.state.label(), "failed", "so a report says so");
    assert!(refused.state.is_failure(), "and the caller sees a failure");
    let detail = detail_of(&refused);
    assert!(
        detail.contains("cannot confirm"),
        "{refused:?} must say what the platform cannot do"
    );
    assert!(
        detail.contains("not removed"),
        "{refused:?} must say the archive was left in place"
    );
    assert_eq!(
        refused.reclaimed_bytes, 0,
        "a refusal frees nothing, so it credits nothing"
    );
}
