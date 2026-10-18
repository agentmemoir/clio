// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that each public export, erasure, and
//! restore route records its durable intent before the effect and its outcome
//! after the effect, and that a restart reconciles one terminal result without
//! repeating an irreversible effect.
//!
//! ## Owns
//! - The intent-failure and outcome-failure arm for each route
//! - The restart-and-reconcile assertions over the pending action
//!
//! ## Does not own
//! - The injected event-writer fault doubles (see `clio-compliance` tests)
//! - The seeded stores and read-back probes (see `fixtures` and `support`)
//!
//! ## Boundary
//!
//! Changes to which routes or fault points this claim proves should normally
//! require changes here. Changes to the durable store contract should not.
//!
//! Keep this module focused on the intent-before-effect and outcome-after-effect
//! claim.

use crate::cli::salt_bytes;
use crate::fixtures::{
    BANK, DENIED_ITEM, DENIED_MARKER, DENIED_SUBJECT, NOW, backup_dir, open_sqlite, scratch,
    seed_sqlite, write_item,
};
use crate::support::{Readback, archive_sqlite, readback, restore_sqlite};
use crate::support_pg::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::AmError;
use clio::store::{
    ComplianceStore, DestinationCustodyStore, DurableRequestStore, LocalEventIntent,
    LocalEventState, LocalEventStore, LocalEventView, PgTestGuard, PostgresStore, ScopedSchema,
    SqliteStore,
};
use clio::{Actor, Bank, ErrorCode, OpContext};
use clio_compliance::subject_request_store_wiring::record_to_row;
use clio_compliance::{
    ArchiveClaim, BindingControls, DeliveryRefusal, DeliveryResult, DeliverySpec, EraseArgs,
    EraseVecSink, ErasureResume, ErasureRetryEnv, JournalState, LocalReviewQuery, MemberFacts,
    RequestCommand, RequestKind, RequestScope, RestoreGuardRefusal, RestoreRequest,
    SubjectRequestLedger, SubjectRequestRecord, begin_restore, deliver_export,
    erase_confirmed_request, load_journal, reconcile_erasure_actions, reconcile_open_destinations,
    reconcile_restore_actions, record_denial, write_review_export,
};
use std::path::Path;

/// The event write the injected fault breaks over the real durable store.
#[derive(Clone, Copy)]
enum Fault {
    /// The intent write fails: the action refuses before any effect.
    Intent,
    /// The outcome write fails: the action stays pending after the effect.
    Outcome,
}

/// A store that delegates to `inner` except for the write `fault` selects.
struct FaultStore<'a> {
    inner: &'a dyn LocalEventStore,
    fault: Fault,
}

impl LocalEventStore for FaultStore<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        match self.fault {
            Fault::Intent => Err(AmError::new(ErrorCode::Internal, "injected intent-write failure")),
            Fault::Outcome => self.inner.record_intent(intent),
        }
    }
    fn record_outcome(&self, _id: &str, _denied: bool, _result: &str) -> Result<(), AmError> {
        match self.fault {
            Fault::Intent => Err(AmError::new(ErrorCode::NotFound, "no intent can exist")),
            Fault::Outcome => {
                Err(AmError::new(ErrorCode::Internal, "injected outcome-write failure"))
            }
        }
    }
    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        self.inner.pending_actions()
    }
    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        self.inner.list_events()
    }
    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, AmError> {
        self.inner.prune_completed_before(cutoff)
    }
}

fn event_rows(events: &dyn LocalEventStore, action_id: &str) -> Vec<LocalEventView> {
    events
        .list_events()
        .expect("read the events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect()
}

fn ctx() -> OpContext {
    OpContext::new(Bank::new(BANK), Actor::Harness)
}

fn accepted() -> BindingControls {
    BindingControls { copy_controls_accepted: true, ..BindingControls::default() }
}

fn spec<'a>(id: &'a str, destination: &'a Path, staging: &'a Path) -> DeliverySpec<'a> {
    DeliverySpec { action_id: id, destination, staging_dir: staging, custody_owner: "clio",
        required_action: "delivery", now: NOW }
}

fn restore_claim(journal: &Path) -> ArchiveClaim {
    match load_journal(journal) {
        JournalState::Current(state) => ArchiveClaim {
            deployment_id: state.deployment_id.clone(),
            generation: state.generation,
            salt_fingerprint: state.salt_fingerprint.clone(),
        },
        other => panic!("expected a current journal, got {other:?}"),
    }
}

fn restore_request<'a>(journal: &'a Path, claim: &'a ArchiveClaim) -> RestoreRequest<'a> {
    RestoreRequest { journal_path: journal, presented: None, claim, members: &[MemberFacts::None],
        actor: "harness", action_id: "restore-act", now: NOW }
}

fn pg_or_skip() -> Option<(PgTestGuard, ScopedSchema, PostgresStore)> {
    let url = pg_url();
    if !require_postgres(postgres_reachable(&url)) {
        return None;
    }
    let guard = lock_pg();
    let schema = ScopedSchema::new(&url);
    let store = schema.open();
    Some((guard, schema, store))
}

fn started_erasure(request_id: &str) -> SubjectRequestRecord {
    let scope = RequestScope::Subject {
        bank_id: BANK.to_owned(),
        subject_id: DENIED_SUBJECT.to_owned(),
    };
    let mut ledger = SubjectRequestLedger::new();
    ledger.register(request_id, RequestKind::Erasure, &scope, &[]).expect("open the request");
    ledger.apply(request_id, &RequestCommand::Begin).expect("begin the work");
    ledger.record(request_id).expect("the record exists").clone()
}

fn seeded_erasure_store(db: &Path, kms: &Path, request_id: &str) -> SqliteStore {
    let store = open_sqlite(db, kms);
    store.save(&record_to_row(&started_erasure(request_id))).expect("persist the request");
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER);
    store
}

fn erase_args<'a>(store: &'a dyn ComplianceStore, ctx: &'a OpContext, sink: &'a EraseVecSink,
    journal: &'a Path, request_id: &'a str, salt: &'a [u8]) -> EraseArgs<'a> {
    EraseArgs { store, journal_path: journal, maint: None, ctx, subject_id: DENIED_SUBJECT,
        legal_basis: "GDPR_ART_17", request_id, reason_code: None, confirm: true, salt,
        timestamp: NOW, sink }
}

/// A failed export intent refuses before staging, the destination, or custody.
#[test]
fn an_export_intent_failure_refuses_before_any_effect() {
    let dir = scratch("export-intent");
    let store = open_sqlite(&dir.join("store.db"), &dir.join("kms"));
    let (destination, staging) = (dir.join("out.bin"), dir.join("staging"));
    let events = FaultStore { inner: &store, fault: Fault::Intent };
    let result = deliver_export(
        &ctx(), &events, &store, &accepted(), &spec("export-act", &destination, &staging),
        b"payload",
    );
    assert_eq!(result, DeliveryResult::Refused(DeliveryRefusal::DurableIntentFailed));
    assert!(!destination.exists() && !staging.exists(), "a refused intent writes nothing");
    let custody = DestinationCustodyStore::load(&store, "export-act").expect("load");
    assert!(custody.is_none(), "no custody row");
    assert!(event_rows(&store, "export-act").is_empty(), "no event row");
}

/// A failed export outcome leaves one effect and one pending action; a restart
/// reconciles it, and a repeat before reconcile never adds a second effect.
#[test]
fn an_export_outcome_failure_reconciles_without_a_second_effect() {
    let dir = scratch("export-outcome");
    let db = dir.join("store.db");
    let (destination, staging) = (dir.join("out.bin"), dir.join("staging"));
    {
        let store = open_sqlite(&db, &dir.join("kms"));
        let events = FaultStore { inner: &store, fault: Fault::Outcome };
        let result = deliver_export(
            &ctx(), &events, &store, &accepted(), &spec("export-act", &destination, &staging),
            b"payload",
        );
        assert!(
            matches!(&result, DeliveryResult::Failed(reason) if reason.contains("not settled")),
            "the outcome fault is reported: {result:?}"
        );
        assert_eq!(event_rows(&store, "export-act")[0].state, LocalEventState::Pending);
        assert_eq!(store.pending_actions().expect("pending"), vec!["export-act".to_owned()]);
        let repeat = deliver_export(
            &ctx(), &events, &store, &accepted(), &spec("export-act", &destination, &staging),
            b"second",
        );
        assert_eq!(repeat, DeliveryResult::Refused(DeliveryRefusal::DurableIntentFailed));
        assert_eq!(std::fs::read(&destination).expect("destination"), b"payload", "no second effect");
    }
    let store = open_sqlite(&db, &dir.join("kms"));
    assert!(
        reconcile_open_destinations(&store, &store).expect("reconcile").is_empty(),
        "the delivered row is not an open destination"
    );
    let rows = event_rows(&store, "export-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(store.pending_actions().expect("pending").is_empty(), "the queue empties");
    assert_eq!(std::fs::read(&destination).expect("destination"), b"payload");
}

/// A failed review-export outcome leaves one file effect and one pending
/// action that reconciles after a restart without redelivering.
#[test]
fn a_review_export_outcome_failure_reconciles_without_redelivery() {
    let dir = scratch("review-outcome");
    let db = dir.join("store.db");
    let (destination, staging) = (dir.join("review.txt"), dir.join("staging"));
    let query = LocalReviewQuery { limit: 100, cursor: None, request_id: None };
    {
        let store = open_sqlite(&db, &dir.join("kms"));
        let events = FaultStore { inner: &store, fault: Fault::Outcome };
        let err = write_review_export(
            &ctx(), &events, &store, &query, &accepted(),
            &spec("review-act", &destination, &staging),
        )
        .expect_err("the outcome fault is reported");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(destination.exists(), "the file effect landed once");
        assert_eq!(event_rows(&store, "review-act")[0].state, LocalEventState::Pending);
        assert_eq!(store.pending_actions().expect("pending"), vec!["review-act".to_owned()]);
    }
    let store = open_sqlite(&db, &dir.join("kms"));
    reconcile_open_destinations(&store, &store).expect("reconcile");
    let rows = event_rows(&store, "review-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(store.pending_actions().expect("pending").is_empty(), "the queue empties");
    assert!(destination.exists(), "the file is never redelivered or removed");
}

/// A failed erasure intent runs no store effect, loses no key, and writes no
/// action row or journal denial.
#[test]
fn an_erasure_intent_failure_refuses_with_no_effect_or_action_row() {
    let dir = scratch("erase-intent");
    let request_id = "erase-intent-req";
    let journal = dir.join("journal.json");
    let store = seeded_erasure_store(&dir.join("store.db"), &dir.join("kms"), request_id);
    let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = salt_bytes();
    let args = erase_args(&store, &ctx, &sink, &journal, request_id, &salt);
    let live = store.erase_bank_set(BANK, DENIED_SUBJECT).expect("live banks");
    let events = FaultStore { inner: &store, fault: Fault::Intent };
    let err = erase_confirmed_request(&store, &events, &store, request_id, &args, &live)
        .expect_err("the intent fault refuses");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(store.list_events().expect("events").is_empty(), "no action row");
    assert!(store.pending_actions().expect("pending").is_empty());
    assert_eq!(store.read_tombstones(BANK, request_id).expect("tombstones").len(), 0, "no effect");
    assert!(!store.subject_dek_destroyed(DENIED_SUBJECT).expect("key"), "no key loss");
    assert!(!journal.exists(), "no journal denial is recorded");
}

/// A failed erasure outcome leaves exactly one effect and one pending action
/// that a restart reconciles without repeating the effect.
#[test]
fn an_erasure_outcome_failure_reconciles_without_a_second_effect() {
    let dir = scratch("erase-outcome");
    let db = dir.join("store.db");
    let request_id = "erase-outcome-req";
    let journal = dir.join("journal.json");
    let salt = salt_bytes();
    let action: String;
    let tombstones: usize;
    {
        let store = seeded_erasure_store(&db, &dir.join("kms"), request_id);
        let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
        let sink = EraseVecSink::default();
        let args = erase_args(&store, &ctx, &sink, &journal, request_id, &salt);
        let live = store.erase_bank_set(BANK, DENIED_SUBJECT).expect("live banks");
        let events = FaultStore { inner: &store, fault: Fault::Outcome };
        erase_confirmed_request(&store, &events, &store, request_id, &args, &live)
            .expect_err("the outcome fault is reported");
        action = store.pending_actions().expect("pending").pop().expect("one pending action");
        let rows = event_rows(&store, &action);
        assert_eq!(rows.len(), 1, "one intent row");
        assert_eq!(rows[0].state, LocalEventState::Pending, "the outcome is unwritten");
        tombstones = store.read_tombstones(BANK, request_id).expect("tombstones").len();
        assert!(tombstones > 0, "the effect committed");
        assert!(store.subject_dek_destroyed(DENIED_SUBJECT).expect("key"), "the key was destroyed");
        let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
        let sink = EraseVecSink::default();
        let args = erase_args(&store, &ctx, &sink, &journal, request_id, &salt);
        let live = store.erase_bank_set(BANK, DENIED_SUBJECT).expect("live banks");
        erase_confirmed_request(&store, &events, &store, request_id, &args, &live)
            .expect_err("the repeat cannot settle");
        assert_eq!(
            store.read_tombstones(BANK, request_id).expect("tombstones").len(), tombstones,
            "the repeat repeats no store effect"
        );
    }
    let store = open_sqlite(&db, &dir.join("kms"));
    let sink = EraseVecSink::default();
    let live = store.erase_bank_set(BANK, DENIED_SUBJECT).expect("live banks");
    let env = ErasureRetryEnv { store: &store, peers: &store, journal_path: &journal, maint: None,
        actor: Actor::Harness, legal_basis: "GDPR_ART_17", salt: &salt, timestamp: NOW,
        sink: &sink };
    let resume = |_: &SubjectRequestRecord| {
        Some(ErasureResume { subject_id: DENIED_SUBJECT.to_owned(), live_banks: live.clone() })
    };
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(report.settled, vec![action.clone()], "the pending action settles");
    assert!(report.pending.is_empty(), "{report:?}");
    assert!(store.pending_actions().expect("pending").is_empty(), "the queue empties");
    let rows = event_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(
        store.read_tombstones(BANK, request_id).expect("tombstones").len(), tombstones,
        "the restart repeats no store effect"
    );
}

/// A failed restore intent refuses before the caller can place anything.
#[test]
fn a_restore_intent_failure_refuses_before_placement() {
    let dir = scratch("restore-intent");
    let journal = dir.join("journal.json");
    record_denial(&journal, "denied-elsewhere", &[]).expect("seed the journal");
    let claim = restore_claim(&journal);
    let store = open_sqlite(&dir.join("events.db"), &dir.join("kms"));
    let events = FaultStore { inner: &store, fault: Fault::Intent };
    let Err(refusal) = begin_restore(&events, &restore_request(&journal, &claim)) else {
        panic!("the intent fault must refuse");
    };
    assert!(matches!(refusal, RestoreGuardRefusal::Event(_)));
    assert!(!dir.join("target.db").exists(), "no guard means no placement can happen");
}

/// A failed restore outcome leaves one placement and one pending action; the
/// restart reconcile denies it without repeating the placement.
#[test]
fn a_restore_outcome_failure_reconciles_without_repeating_placement() {
    let dir = scratch("restore-outcome");
    let (src_db, src_kms) = (dir.join("src.db"), dir.join("src.kms"));
    seed_sqlite(&src_db, &src_kms, &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)]);
    let archive = archive_sqlite(&backup_dir(&dir), &src_db, &src_kms, "restore-outcome");
    let journal = dir.join("journal.json");
    record_denial(&journal, "denied-elsewhere", &[]).expect("seed the journal");
    let claim = restore_claim(&journal);
    let (db, target_db, target_kms) =
        (dir.join("events.db"), dir.join("target.db"), dir.join("target.kms"));
    {
        let store = open_sqlite(&db, &dir.join("kms"));
        let events = FaultStore { inner: &store, fault: Fault::Outcome };
        let guard = begin_restore(&events, &restore_request(&journal, &claim)).expect("intent lands");
        // The effect the guard does not perform: the caller places the archive.
        restore_sqlite(&archive, &target_db, &target_kms, true);
        guard.settle(&events, true, "placed").expect_err("the outcome write fails");
        let rows = event_rows(&store, "restore-act");
        assert_eq!(rows.len(), 1, "one intent row");
        assert_eq!(rows[0].state, LocalEventState::Pending, "the outcome is unwritten");
    }
    assert_eq!(
        readback(&open_sqlite(&target_db, &target_kms), DENIED_ITEM), Readback::Readable,
        "the placement landed exactly once"
    );
    let store = open_sqlite(&db, &dir.join("kms"));
    assert_eq!(
        reconcile_restore_actions(&store).expect("reconcile"), vec!["restore-act".to_owned()],
        "the pending restore settles"
    );
    let rows = event_rows(&store, "restore-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert!(rows[0].result.is_some(), "the reconcile names a content-free reason");
    assert!(store.pending_actions().expect("pending").is_empty(), "the queue empties");
    let events = FaultStore { inner: &store, fault: Fault::Outcome };
    assert!(
        begin_restore(&events, &restore_request(&journal, &claim)).is_err(),
        "the denied action never resumes"
    );
    assert_eq!(
        readback(&open_sqlite(&target_db, &target_kms), DENIED_ITEM), Readback::Readable,
        "the repeat repeats no placement"
    );
}

/// A failed Postgres export outcome leaves one effect and one pending action
/// that reconciles after a restart without a second destination write.
#[test]
fn a_postgres_export_outcome_failure_reconciles_without_a_second_effect() {
    let Some((_guard, schema, store)) = pg_or_skip() else {
        return;
    };
    let dir = scratch("pg-export-outcome");
    let (destination, staging) = (dir.join("out.bin"), dir.join("staging"));
    {
        let events = FaultStore { inner: &store, fault: Fault::Outcome };
        let result = deliver_export(
            &ctx(), &events, &store, &accepted(), &spec("pg-export-act", &destination, &staging),
            b"payload",
        );
        assert!(
            matches!(&result, DeliveryResult::Failed(reason) if reason.contains("not settled")),
            "the outcome fault is reported: {result:?}"
        );
    }
    assert_eq!(
        std::fs::read(&destination).expect("destination"), b"payload",
        "exactly one destination effect"
    );
    assert_eq!(event_rows(&store, "pg-export-act")[0].state, LocalEventState::Pending);
    assert_eq!(store.pending_actions().expect("pending"), vec!["pg-export-act".to_owned()]);
    drop(store);
    let store = schema.open();
    assert!(
        reconcile_open_destinations(&store, &store).expect("reconcile").is_empty(),
        "the delivered row is not an open destination"
    );
    let rows = event_rows(&store, "pg-export-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(store.pending_actions().expect("pending").is_empty(), "the queue empties");
    assert_eq!(std::fs::read(&destination).expect("destination"), b"payload");
}
