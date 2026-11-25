// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one question: what a removal reports when the
//! archive it removed is still held by another hard link, so the run freed no
//! blocks even though the entry it named is gone.
//!
//! ## Owns
//! - The byte credit of a removal whose entry had more than one link
//!
//! ## Does not own
//! - The plain removal and byte cases (see `prune_delete_tests`)
//! - The identity confirm (see `prune_delete::prune_delete_dir::tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{candidate, detail_of, file_len, plan_with, states_of};
use super::*;
use crate::SNAPSHOT_EXTENSION;
use crate::fixtures::Scratch;

/// An archive another hard link still holds is reported removed and credits no
/// byte, because the unlink freed none.
///
/// A second name for the same inode means the blocks stay allocated after the
/// archive's own name goes. The entry really was removed, so the state is
/// `Deleted`, but the run freed nothing, so `reclaimed_bytes` is zero and the
/// detail says why. The remaining figure stays at the listing's total, because an
/// uncredited entry must stay in it: a report that dropped the bytes would claim
/// space that is still on disk.
#[cfg(unix)]
#[test]
fn an_archive_another_hard_link_still_holds_is_removed_and_credits_no_byte() {
    let backup = Scratch::new("prune-hard-link-backup");
    let other_name = Scratch::new("prune-hard-link-elsewhere");
    let archive = backup.path().join(format!("linked{SNAPSHOT_EXTENSION}"));
    std::fs::write(&archive, b"an archive with a second name").expect("write the archive");
    let bytes = file_len(&archive);
    let held_elsewhere = other_name.join("second-name.cliobak.tar.gz");
    std::fs::hard_link(&archive, &held_elsewhere).expect("a second hard link to the archive");
    let plan = plan_with(vec![candidate("linked", &archive, bytes)], 0);
    assert_eq!(plan.listed_bytes, bytes, "the listing measured the archive");

    let report = delete(backup.path(), &plan);

    assert_eq!(
        states_of(&report),
        vec![("linked", CandidateState::Deleted)],
        "the entry really was removed from the backup directory"
    );
    let outcome = &report.candidates[0];
    assert_eq!(
        outcome.reclaimed_bytes, 0,
        "the unlink freed no block, because the second name still holds the file"
    );
    assert!(
        detail_of(outcome).contains("another link"),
        "{outcome:?} must say why it credited nothing"
    );
    assert_eq!(
        report.reclaimed_bytes, 0,
        "the run as a whole freed nothing"
    );
    assert_eq!(
        report.remaining_bytes, plan.listed_bytes,
        "an uncredited entry must stay in the remaining figure"
    );
    assert!(!archive.exists(), "the archive's own name is gone");
    assert!(
        held_elsewhere.is_file(),
        "the second name still holds the file"
    );
    assert_eq!(
        file_len(&held_elsewhere),
        bytes,
        "and it keeps every byte of it"
    );
}
