// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing what containment means when the path
//! spelling and the directory on disk disagree: a backup directory that is
//! itself a symbolic link, a candidate that reaches an archive through a linked
//! parent, and a backup directory that cannot be resolved at all.
//!
//! ## Owns
//! - The resolved-root containment assertions: what is removed, what is not, and
//!   what an unresolvable root reports
//!
//! ## Does not own
//! - The plain removal, byte, symlink, and already-gone cases (see
//!   `prune_delete_tests`)
//! - The containment rule itself (see `prune_delete`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{archive_at, candidate, detail_of, file_len, plan_over, plan_with, states_of};
use super::*;
use crate::SNAPSHOT_EXTENSION;
use crate::fixtures::Scratch;

/// The size of each same-named file the second directory holds.
const DECOY_BYTES: u64 = 24;

/// A backup directory named by a symbolic link removes the archive inside the
/// directory the link names, and reaches nothing outside it.
///
/// The caller passes the link, so the run must resolve it once and unlink
/// through the resolved directory. A path-value comparison alone would have no
/// way to tell these two directories apart.
#[cfg(unix)]
#[test]
fn a_backup_directory_that_is_a_symlink_deletes_nothing_outside_it() {
    let scratch = Scratch::new("prune-link-root");
    let elsewhere = Scratch::new("prune-link-root-outside");
    let decoy = elsewhere.join("decoy.cliobak.tar.gz");
    std::fs::write(&decoy, b"not yours").expect("write a file outside the backup directory");
    let decoy_bytes = file_len(&decoy);
    let real = scratch.join("real");
    std::fs::create_dir(&real).expect("the directory the link names");
    let newest = archive_at(&scratch, 91, "2026-05-05T00:00:00Z", "newest");
    let second = archive_at(&scratch, 92, "2026-04-04T00:00:00Z", "second");
    let oldest = archive_at(&scratch, 93, "2026-03-03T00:00:00Z", "third");
    for archive in [&newest, &second, &oldest] {
        let name = archive.file_name().expect("an archive file name");
        std::fs::rename(archive, real.join(name))
            .expect("move the archive into the real directory");
    }
    let link = scratch.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("link to the real directory");
    let plan = plan_over(&link, 1);
    assert_eq!(plan.candidates.len(), 2, "the two older archives");
    let expected_bytes = file_len(&real.join(second.file_name().expect("a name")))
        + file_len(&real.join(oldest.file_name().expect("a name")));

    let report = delete(&link, &plan);

    assert_eq!(
        states_of(&report),
        plan.candidates
            .iter()
            .map(|candidate| (candidate.id.as_str(), CandidateState::Deleted))
            .collect::<Vec<_>>(),
        "both candidates went through the link"
    );
    assert_eq!(
        report.reclaimed_bytes, expected_bytes,
        "the bytes credited are the sizes of the two entries that really went"
    );
    for outcome in &report.candidates {
        assert_eq!(
            outcome.reclaimed_bytes, outcome.listed_bytes,
            "each credited size is the entry's own: {outcome:?}"
        );
        assert_eq!(detail_of(outcome), "", "a plain removal has no reason");
    }
    assert_eq!(
        report.remaining_bytes,
        plan.listed_bytes - expected_bytes,
        "the remaining figure agrees with what was freed"
    );
    assert!(
        !real.join(second.file_name().expect("a name")).exists(),
        "the second archive went from the real directory"
    );
    assert!(
        !real.join(oldest.file_name().expect("a name")).exists(),
        "the third archive went too"
    );
    let kept = real.join(newest.file_name().expect("a name"));
    assert!(kept.is_file(), "the kept archive stays where it was");
    assert!(
        decoy.is_file(),
        "the file outside the directory is untouched"
    );
    assert_eq!(file_len(&decoy), decoy_bytes, "and it keeps every byte");
    assert_eq!(
        report.backup_dir, link,
        "the report names the path the caller passed"
    );
}

/// A candidate that reaches an archive through a linked parent is refused, and
/// the archive is left where it is.
///
/// The linked parent is a second name for the backup directory, so the file it
/// names really is inside the backup directory on disk. The run still refuses
/// it: only a bare file name directly under the caller's own backup directory is
/// a candidate, and a link in the parent is not that.
#[cfg(unix)]
#[test]
fn a_candidate_whose_parent_is_a_symlink_is_refused() {
    let backup = Scratch::new("prune-linked-parent-backup");
    let elsewhere = Scratch::new("prune-linked-parent-alias");
    let archive = archive_at(&backup, 94, "2026-05-05T00:00:00Z", "newest");
    let bytes = file_len(&archive);
    let alias = elsewhere.join("alias");
    std::os::unix::fs::symlink(backup.path(), &alias).expect("link to the backup directory");
    let through_alias = alias.join(archive.file_name().expect("an archive file name"));
    let plan = plan_with(vec![candidate("through-alias", &through_alias, bytes)], 0);

    let report = delete(backup.path(), &plan);

    let outcome = &report.candidates[0];
    assert_eq!(outcome.state, CandidateState::OutsideDirectory);
    assert_eq!(outcome.state.label(), "outside_directory", "as one word");
    assert!(!outcome.state.is_failure(), "a refusal is not a failure");
    assert!(
        detail_of(outcome).contains("is not a file inside"),
        "{:?}",
        outcome.detail
    );
    assert_eq!(report.reclaimed_bytes, 0, "nothing was unlinked");
    assert!(archive.is_file(), "the archive is still there");
    assert_eq!(file_len(&archive), bytes, "and it keeps every byte");
}

/// A backup directory repointed under the run removes nothing from the new
/// target.
///
/// The link is moved in the one gap the held descriptor and the identity confirm
/// close. The candidate inspected before the move is removed from the directory
/// the run holds open, which is the directory the plan was built over. The
/// candidate after it is refused, because the caller's path no longer resolves to
/// that directory. The pre-fix code compares the parent's spelling only and then
/// removes the candidate's path, so it follows the repointed link and takes the
/// same-named file in the second directory instead.
#[cfg(unix)]
#[test]
fn a_backup_directory_repointed_during_the_run_deletes_nothing_from_the_new_target() {
    use super::prune_delete_dir::gap;
    use std::cell::Cell;
    let scratch = Scratch::new("prune-repoint-midrun");
    let decoys = Scratch::new("prune-repoint-decoys");
    let real = scratch.join("real");
    std::fs::create_dir(&real).expect("the directory the link names");
    let newest = archive_at(&scratch, 95, "2026-05-05T00:00:00Z", "newest");
    let second = archive_at(&scratch, 96, "2026-04-04T00:00:00Z", "second");
    let oldest = archive_at(&scratch, 97, "2026-03-03T00:00:00Z", "third");
    let names: Vec<std::ffi::OsString> = [&newest, &second, &oldest]
        .iter()
        .map(|archive| {
            archive
                .file_name()
                .expect("an archive file name")
                .to_owned()
        })
        .collect();
    for (archive, name) in [&newest, &second, &oldest].iter().zip(&names) {
        std::fs::rename(archive, real.join(name))
            .expect("move the archive into the real directory");
    }
    let link = scratch.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("link to the real directory");
    let plan = plan_over(&link, 1);
    assert_eq!(
        plan.candidates.len(),
        2,
        "the two older archives are the candidates"
    );
    // One same-named file per archive in the second directory, so a removal that
    // followed the link again would take it and leave the real one behind.
    for name in &names {
        let decoy = decoys.path().join(name);
        std::fs::write(&decoy, [b'x'; DECOY_BYTES as usize]).expect("write the decoy");
    }
    // The link moves under the run, in the gap the confirm closes.
    let spare = scratch.join("spare");
    std::os::unix::fs::symlink(decoys.path(), &spare).expect("a second link, not yet in place");
    let fired = Cell::new(false);
    let link_for_hook = link.clone();
    let _hook = gap::set(move |_dir, _name| {
        if !fired.replace(true) {
            std::fs::rename(&spare, &link_for_hook).expect("repoint the link in the gap");
        }
    });

    let report = delete(&link, &plan);

    assert_eq!(
        states_of(&report),
        vec![
            (plan.candidates[0].id.as_str(), CandidateState::Deleted),
            (
                plan.candidates[1].id.as_str(),
                CandidateState::OutsideDirectory
            ),
        ],
        "the held directory removes one, and the repointed path is refused"
    );
    assert!(
        detail_of(&report.candidates[1]).contains("is not a file inside"),
        "{:?}",
        report.candidates[1].detail
    );
    for (index, name) in names.iter().enumerate().skip(1) {
        let decoy = decoys.path().join(name);
        assert!(
            decoy.is_file(),
            "candidate {index} must leave the second directory's file alone"
        );
        assert_eq!(
            file_len(&decoy),
            DECOY_BYTES,
            "and it keeps every byte of its own"
        );
    }
    assert!(
        !real.join(&names[1]).exists(),
        "the removed candidate left the directory the run holds open"
    );
    assert!(
        real.join(&names[2]).is_file(),
        "the refused candidate is still where it was"
    );
    assert!(
        real.join(&names[0]).is_file(),
        "the kept archive stays where it was"
    );
    assert_eq!(
        report.reclaimed_bytes, plan.candidates[0].listed_bytes,
        "only the candidate the held descriptor removed was credited"
    );
    assert_eq!(
        report.remaining_bytes,
        plan.listed_bytes - report.reclaimed_bytes,
        "the remaining figure agrees with what was freed"
    );
}

#[test]
fn an_unresolvable_backup_directory_fails_every_candidate_and_removes_nothing() {
    let scratch = Scratch::new("prune-unresolvable-root");
    let kept = Scratch::new("prune-unresolvable-kept");
    let orphan = kept.path().join(format!("orphan{SNAPSHOT_EXTENSION}"));
    std::fs::write(&orphan, b"someone else's file").expect("write a file to leave alone");
    let orphan_bytes = file_len(&orphan);
    let plain_file = scratch.join("a-regular-file");
    std::fs::write(&plain_file, b"not a directory").expect("write a regular file");
    let missing = scratch.join("not-a-directory");
    let under_missing = missing.join(format!("under-missing{SNAPSHOT_EXTENSION}"));
    let under_file = plain_file.join(format!("under-file{SNAPSHOT_EXTENSION}"));
    let candidates = vec![
        candidate("under-missing", &under_missing, orphan_bytes),
        candidate("orphan", &orphan, orphan_bytes),
        candidate("under-file", &under_file, 0),
    ];
    let plan = plan_with(candidates, 0);

    let report = delete(&missing, &plan);

    let unresolved =
        std::fs::canonicalize(missing.as_path()).expect_err("the directory does not exist");
    assert_eq!(
        states_of(&report),
        vec![
            ("under-missing", CandidateState::Failed),
            ("orphan", CandidateState::Failed),
            ("under-file", CandidateState::Failed),
        ],
        "an unresolved root fails every candidate"
    );
    for outcome in &report.candidates {
        let detail = detail_of(outcome);
        assert!(
            detail.contains("could not be resolved"),
            "{outcome:?} must say the directory was not resolved"
        );
        assert!(
            detail.contains(&missing.display().to_string()),
            "{outcome:?} must name the directory it could not resolve"
        );
        assert!(
            detail.contains(&unresolved.to_string()),
            "{outcome:?} must carry the underlying error"
        );
    }
    assert!(report.has_failures(), "the caller sees the failure");
    assert_eq!(report.candidates[0].state.label(), "failed", "as one word");
    assert_eq!(report.reclaimed_bytes, 0, "nothing was unlinked");
    assert_eq!(
        report.remaining_bytes, plan.listed_bytes,
        "every byte is still there"
    );
    assert!(orphan.is_file(), "the file it was told about is untouched");
    assert_eq!(file_len(&orphan), orphan_bytes, "and it keeps its bytes");
    assert_eq!(
        report.backup_dir, missing,
        "the report names the path the caller passed"
    );
}
