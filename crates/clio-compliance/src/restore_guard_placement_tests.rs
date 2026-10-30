// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the placement-time
//! copy-custody refusal: `begin_restore` refuses under the journal lock when
//! the copy-custody facts refuse, and records exactly one denied outcome.
//!
//! ## Owns
//! - The digest-mismatch refusal through `begin_restore`'s placement branch
//!
//! ## Does not own
//! - The authority refusal through the same branch (see `restore_guard_tests`)
//! - The gate's own refusal set (see `restore_guard_custody_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the placement-time custody refusal or its recorded
//! outcome changes.
//!
//! Test-only. Never compiled into production builds.

use super::restore_guard_tests::{
    NOW, Scratch, claim_for, denied_journal, open_sqlite, passing, restore_rows,
};
use super::{CopyCustodyFacts, RestoreGuardRefusal, RestoreRequest, begin_restore};
use crate::erasure_journal::{JournalState, MemberFacts};
use crate::subject_request_events::RestoreRefusal;
use clio_store::{LocalEventState, LocalEventStore};
use clio_types::ErrorCode;

/// The digest the copy presents, different from the digest its custody row
/// records.
const PRESENTED: &str = "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

#[test]
fn begin_restore_refuses_placement_and_records_one_denial_when_the_copy_digest_mismatches() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let presented = JournalState::Current(journal.clone());
    let events = open_sqlite();
    let custody = CopyCustodyFacts {
        copy_digest: PRESENTED,
        ..passing()
    };
    let request = RestoreRequest {
        journal_path: &path,
        presented: Some(&presented),
        claim: &claim_for(&journal),
        members: &[MemberFacts::None],
        copy_custody: custody,
        actor: "actor",
        action_id: "act-custody",
        now: NOW,
    };
    let Err(refusal) = begin_restore(&events, &request) else {
        panic!("the copy-custody check must refuse placement");
    };
    assert!(matches!(
        refusal,
        RestoreGuardRefusal::Custody(RestoreRefusal::CopyDigestMismatch)
    ));
    assert_eq!(refusal.code(), ErrorCode::Forbidden);
    let rows = restore_rows(&events);
    assert_eq!(rows.len(), 1, "exactly one restore event is recorded");
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert_eq!(rows[0].result.as_deref(), Some("refused before placement"));
    let pending = events.pending_actions().expect("pending");
    assert!(pending.is_empty(), "no pending action remains");
}
