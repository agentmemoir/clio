// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the confirmed prune on a platform with no
//! directory descriptor: every candidate is inspected, refused, and kept.
//!
//! ## Owns
//! - The non-unix `delete` outcome: every candidate refused, every file kept
//!
//! ## Does not own
//! - The refusal's own wording and state (see `prune_delete_tests`)
//! - The unix removal paths (see `prune_delete_dir_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. A compile of the non-unix
//! body cannot prove this dispatch, so this module runs the real `delete` entry
//! point on the platform itself: a lost refusal call fails here.

use super::tests::{candidate, detail_of, file_len, plan_with, states_of};
use super::{CandidateState, delete};
use crate::fixtures::{Scratch, dir_entries};

/// A confirmed prune inspects every candidate on this platform, refuses each
/// one, and leaves both archives on disk.
#[test]
fn a_confirmed_run_refuses_every_candidate_and_keeps_the_files() {
    let scratch = Scratch::new("prune-non-unix");
    let first = scratch.join(&format!("first{}", crate::SNAPSHOT_EXTENSION));
    let second = scratch.join(&format!("second{}", crate::SNAPSHOT_EXTENSION));
    std::fs::write(&first, b"the first archive").expect("write the first archive");
    std::fs::write(&second, b"the second archive").expect("write the second archive");
    let first_len = file_len(&first);
    let second_len = file_len(&second);
    let plan = plan_with(
        vec![
            candidate("first", &first, first_len),
            candidate("second", &second, second_len),
        ],
        0,
    );

    let report = delete(scratch.path(), &plan);

    assert!(!report.dry_run, "a confirmed run says so");
    assert_eq!(
        states_of(&report),
        vec![
            ("first", CandidateState::Failed),
            ("second", CandidateState::Failed),
        ],
        "every candidate is refused, in the plan's order"
    );
    for outcome in &report.candidates {
        let detail = detail_of(outcome);
        assert!(
            detail.contains("cannot confirm"),
            "{outcome:?} must say what the platform cannot do"
        );
        assert!(
            detail.contains("not removed"),
            "{outcome:?} must say the archive was left in place"
        );
        assert_eq!(outcome.reclaimed_bytes, 0, "a refusal frees nothing");
    }
    assert_eq!(report.reclaimed_bytes, 0, "the run freed nothing");
    assert!(report.has_failures(), "the run reports a degraded outcome");
    assert_eq!(file_len(&first), first_len, "the first archive stays");
    assert_eq!(file_len(&second), second_len, "the second archive stays");
    assert_eq!(
        dir_entries(scratch.path()),
        vec![
            format!("first{}", crate::SNAPSHOT_EXTENSION),
            format!("second{}", crate::SNAPSHOT_EXTENSION),
        ],
        "nothing else was written and nothing was removed"
    );
}
