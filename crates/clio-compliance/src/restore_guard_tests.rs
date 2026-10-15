// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the guarded restore: the
//! planning and placement authority checks, the durable intent before
//! placement, the journal lock the placement guard holds, and the settle. The
//! journal resolution and the under-the-lock reconcile live in
//! `restore_guard_journal_tests`.
//!
//! ## Owns
//! - The scratch journal fixture and the injected intent-failure event store
//! - The authority refusal assertions and the placement, lock, and settle
//!   assertions
//!
//! ## Does not own
//! - The guarded restore itself (see `restore_guard.rs`)
//! - The journal authority rules (see `erasure_journal.rs`)
//! - The durable event writer (`clio-store` owns it)
//!
//! ## Boundary
//!
//! Tests here break when the restore sequence or the refusal set changes.
//!
//! Test-only. Never compiled into production builds.

use super::{
    JournalHold, RESTORE_EVENT_KIND, RestoreGuardRefusal, RestoreRequest, begin_restore,
    check_restore_at_plan,
};
use crate::erasure_journal::{
    ArchiveClaim, AuthorityRefusal, ErasureJournal, JournalState, MemberFacts, load_journal,
    record_denial,
};
use clio_store::{
    DekProvider, LocalDevKms, LocalEventIntent, LocalEventState, LocalEventStore, LocalEventView,
    SqliteStore,
};
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The time every test passes as the event time, so no test reads the clock.
pub(crate) const NOW: &str = "2026-01-01T00:00:00Z";

/// The subject the fixture journal denies.
const DENIED: &str = "subject-denied";

/// The subject the fixture members reference without a denial.
const CLEAR: &str = "subject-clear";

/// In-memory SQLite store (the `local_action_probe_tests` fixture shape).
pub(crate) fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

/// A store whose intent write always fails: the injected intent fault.
struct IntentWriteFails;

impl LocalEventStore for IntentWriteFails {
    fn record_intent(&self, _intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "injected intent-write failure",
        ))
    }

    fn record_outcome(
        &self,
        _action_id: &str,
        _denied: bool,
        _result: &str,
    ) -> Result<(), AmError> {
        Err(AmError::new(ErrorCode::NotFound, "no intent can exist"))
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        Ok(Vec::new())
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        Ok(Vec::new())
    }

    fn prune_completed_before(&self, _cutoff: &str) -> Result<usize, AmError> {
        Ok(0)
    }
}

/// A scratch directory holding a journal path, removed on drop.
pub(crate) struct Scratch {
    root: PathBuf,
}

impl Scratch {
    pub(crate) fn new() -> Self {
        let root =
            std::env::temp_dir().join(crate::test_unique::unique("clio-compliance-restore-guard"));
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Self { root }
    }

    pub(crate) fn journal(&self) -> PathBuf {
        self.root.join("erasure_journal.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The current journal at `path`, or a panic naming the state.
fn current(path: &Path) -> ErasureJournal {
    match load_journal(path) {
        JournalState::Current(journal) => journal,
        other => panic!("expected a current journal, got {other:?}"),
    }
}

/// Write a journal with one denial and read it back, with its path.
pub(crate) fn denied_journal(scratch: &Scratch) -> (PathBuf, ErasureJournal) {
    let path = scratch.journal();
    record_denial(&path, DENIED, &[]).expect("record the denial");
    let journal = current(&path);
    (path, journal)
}

/// A claim for the authority the journal itself records.
pub(crate) fn claim_for(journal: &ErasureJournal) -> ArchiveClaim {
    ArchiveClaim {
        deployment_id: journal.deployment_id.clone(),
        generation: journal.generation,
        salt_fingerprint: journal.salt_fingerprint.clone(),
    }
}

/// The `restore_guard` rows of an event store, in store order.
pub(crate) fn restore_rows(events: &dyn LocalEventStore) -> Vec<LocalEventView> {
    events
        .list_events()
        .expect("list events")
        .into_iter()
        .filter(|event| event.kind == RESTORE_EVENT_KIND)
        .collect()
}

// ---------------------------------------------------------------------------
// The plan and placement authority checks
// ---------------------------------------------------------------------------

#[test]
fn plan_check_passes_for_a_same_deployment_claim_with_non_denied_members() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let presented = JournalState::Current(journal.clone());
    let members = [
        MemberFacts::None,
        MemberFacts::Reference {
            subject_hash: CLEAR.to_owned(),
        },
    ];
    assert!(check_restore_at_plan(&path, Some(&presented), &claim_for(&journal), &members).is_ok());
}

#[test]
fn plan_check_refuses_a_denied_subject_member() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let members = [MemberFacts::Reference {
        subject_hash: DENIED.to_owned(),
    }];
    assert_eq!(
        check_restore_at_plan(&path, None, &claim_for(&journal), &members),
        Err(AuthorityRefusal::DeniedSubject)
    );
}

#[test]
fn plan_check_refuses_an_unknown_member() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    assert_eq!(
        check_restore_at_plan(&path, None, &claim_for(&journal), &[MemberFacts::Unknown]),
        Err(AuthorityRefusal::UnknownMember)
    );
}

#[test]
fn plan_check_refuses_a_missing_journal() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let claim = ArchiveClaim {
        deployment_id: "some-deployment".to_owned(),
        generation: 0,
        salt_fingerprint: Some("fp-live".to_owned()),
    };
    assert_eq!(
        check_restore_at_plan(&path, None, &claim, &[MemberFacts::None]),
        Err(AuthorityRefusal::CurrentMissing)
    );
}

#[test]
fn plan_check_defaults_to_the_live_authority_when_none_is_presented() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    assert!(check_restore_at_plan(&path, None, &claim_for(&journal), &[MemberFacts::None]).is_ok());
}

#[test]
fn plan_check_refuses_a_damaged_journal() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    std::fs::write(&path, b"{not a journal").expect("plant a damaged journal");
    let claim = ArchiveClaim {
        deployment_id: "some-deployment".to_owned(),
        generation: 0,
        salt_fingerprint: Some("fp-live".to_owned()),
    };
    assert_eq!(
        check_restore_at_plan(&path, None, &claim, &[MemberFacts::None]),
        Err(AuthorityRefusal::CurrentDamaged)
    );
}

#[test]
fn plan_check_refuses_a_claim_for_another_deployment() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let claim = ArchiveClaim {
        deployment_id: "other-deployment".to_owned(),
        generation: journal.generation,
        salt_fingerprint: Some("fp-live".to_owned()),
    };
    assert_eq!(
        check_restore_at_plan(&path, None, &claim, &[MemberFacts::None]),
        Err(AuthorityRefusal::ClaimOtherDeployment)
    );
}

#[test]
fn plan_check_refuses_a_claim_for_a_newer_generation() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let claim = ArchiveClaim {
        deployment_id: journal.deployment_id.clone(),
        generation: journal.generation + 1,
        salt_fingerprint: Some("fp-live".to_owned()),
    };
    assert_eq!(
        check_restore_at_plan(&path, None, &claim, &[MemberFacts::None]),
        Err(AuthorityRefusal::ClaimNewerGeneration)
    );
}

// ---------------------------------------------------------------------------
// Begin, lock, and settle
// ---------------------------------------------------------------------------

#[test]
fn begin_restore_releases_the_lock_when_the_intent_write_fails() {
    let scratch = Scratch::new();
    let path = scratch.journal();
    let claim = ArchiveClaim {
        deployment_id: "some-deployment".to_owned(),
        generation: 0,
        salt_fingerprint: Some("fp-live".to_owned()),
    };
    let request = RestoreRequest {
        journal_path: &path,
        presented: None,
        claim: &claim,
        members: &[],
        actor: "actor",
        action_id: "act-intent",
        now: NOW,
    };
    let Err(refusal) = begin_restore(&IntentWriteFails, &request) else {
        panic!("the intent fault must refuse");
    };
    assert!(matches!(refusal, RestoreGuardRefusal::Event(_)));
    // The lock is taken first and released on the failed begin: a fresh hold on
    // the same path succeeds.
    assert!(JournalHold::acquire(&path).is_ok());
}

#[test]
fn begin_restore_refuses_placement_and_records_one_denial_when_the_authority_refuses() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let presented = JournalState::Current(journal.clone());
    let claim = ArchiveClaim {
        deployment_id: "other-deployment".to_owned(),
        generation: journal.generation,
        salt_fingerprint: Some("fp-live".to_owned()),
    };
    let events = open_sqlite();
    let request = RestoreRequest {
        journal_path: &path,
        presented: Some(&presented),
        claim: &claim,
        members: &[MemberFacts::None],
        actor: "actor",
        action_id: "act-refused",
        now: NOW,
    };
    let Err(refusal) = begin_restore(&events, &request) else {
        panic!("the authority check must refuse placement");
    };
    assert!(matches!(
        refusal,
        RestoreGuardRefusal::Authority(AuthorityRefusal::ClaimOtherDeployment)
    ));
    assert_eq!(refusal.code(), ErrorCode::Forbidden);
    let rows = restore_rows(&events);
    assert_eq!(rows.len(), 1, "exactly one restore event is recorded");
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert_eq!(rows[0].result.as_deref(), Some("refused before placement"));
    let pending = events.pending_actions().expect("pending");
    assert!(pending.is_empty(), "no pending action remains");
}

#[test]
fn begin_restore_defaults_to_the_live_authority_when_none_is_presented() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let events = open_sqlite();
    let request = RestoreRequest {
        journal_path: &path,
        presented: None,
        claim: &claim_for(&journal),
        members: &[MemberFacts::None],
        actor: "actor",
        action_id: "act-none",
        now: NOW,
    };
    begin_restore(&events, &request).expect("a same-host restore presents the live authority");
}

#[test]
fn begin_restore_holds_the_journal_lock_until_settle() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let presented = JournalState::Current(journal.clone());
    let events = open_sqlite();
    let request = RestoreRequest {
        journal_path: &path,
        presented: Some(&presented),
        claim: &claim_for(&journal),
        members: &[MemberFacts::None],
        actor: "actor",
        action_id: "act-lock",
        now: NOW,
    };
    let guard = match begin_restore(&events, &request) {
        Ok(guard) => guard,
        Err(refusal) => panic!("begin refused: {refusal}"),
    };
    let blocked = JournalHold::acquire(&path);
    assert!(blocked.is_err(), "the guard must hold the journal lock");
    drop(blocked);
    guard.settle(&events, true, "placed").expect("settle");
    assert!(
        JournalHold::acquire(&path).is_ok(),
        "settle releases the journal lock"
    );
}

#[test]
fn settle_records_one_allowed_outcome() {
    let scratch = Scratch::new();
    let (path, journal) = denied_journal(&scratch);
    let presented = JournalState::Current(journal.clone());
    let events = open_sqlite();
    let members = [MemberFacts::Reference {
        subject_hash: CLEAR.to_owned(),
    }];
    let request = RestoreRequest {
        journal_path: &path,
        presented: Some(&presented),
        claim: &claim_for(&journal),
        members: &members,
        actor: "actor",
        action_id: "act-settle",
        now: NOW,
    };
    let guard = match begin_restore(&events, &request) {
        Ok(guard) => guard,
        Err(refusal) => panic!("begin refused: {refusal}"),
    };
    assert_eq!(guard.action_id(), "act-settle");
    guard.settle(&events, true, "placed").expect("settle");
    let rows = restore_rows(&events);
    assert_eq!(rows.len(), 1, "exactly one restore event is recorded");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some("placed"));
}
