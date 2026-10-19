// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared event-writer fault doubles and the
//! intent-failure arm of each export, erasure, and restore route: a fault at the
//! intent write refuses before any effect.
//!
//! ## Owns
//! - The `Fault`/`FaultStore` doubles the sibling reconcile module reuses
//! - The intent-failure arm for each route
//!
//! ## Does not own
//! - The outcome-failure and restart-reconcile arms (see
//!   `scenario_intent_outcome_faults_reconcile`)
//! - The seeded stores and read-back probes (see `fixtures` and `support`)
//!
//! ## Boundary
//!
//! Changes to which routes or fault points this claim proves should normally
//! require changes here. Changes to the durable store contract should not.
//!
//! Keep this module focused on the shared fault doubles and the intent-failure
//! arm.

use crate::cli::salt_bytes;
use crate::fixtures::{
    BANK, DENIED_ITEM, DENIED_MARKER, DENIED_SUBJECT, NOW, open_sqlite, scratch, write_item,
};
use crate::support_pg::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::AmError;
use clio::store::{
    ComplianceStore, DestinationCustodyStore, DurableRequestStore, LocalEventIntent,
    LocalEventStore, LocalEventView, PgTestGuard, PostgresStore, ScopedSchema, SqliteStore,
};
use clio::{Actor, Bank, ErrorCode, OpContext};
use clio_compliance::subject_request_store_wiring::record_to_row;
use clio_compliance::{
    ArchiveClaim, BindingControls, DeliveryRefusal, DeliveryResult, DeliverySpec, EraseArgs,
    EraseVecSink, JournalState, MemberFacts, RequestCommand, RequestKind, RequestScope,
    RestoreGuardRefusal, RestoreRequest, SubjectRequestLedger, SubjectRequestRecord, begin_restore,
    deliver_export, erase_confirmed_request, load_journal, record_denial,
};
use std::path::Path;

/// The message the injected outcome-write failure carries.
pub(crate) const INJECTED_OUTCOME_FAILURE: &str = "injected outcome-write failure";

/// The event write the injected fault breaks over the real durable store.
#[derive(Clone, Copy)]
pub(crate) enum Fault {
    /// The intent write fails: the action refuses before any effect.
    Intent,
    /// The outcome write fails: the action stays pending after the effect.
    Outcome,
}

/// A store that delegates to `inner` except for the write `fault` selects.
pub(crate) struct FaultStore<'a> {
    pub(crate) inner: &'a dyn LocalEventStore,
    pub(crate) fault: Fault,
}

impl LocalEventStore for FaultStore<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        match self.fault {
            Fault::Intent => Err(AmError::new(
                ErrorCode::Internal,
                "injected intent-write failure",
            )),
            Fault::Outcome => self.inner.record_intent(intent),
        }
    }
    fn record_outcome(&self, _id: &str, _denied: bool, _result: &str) -> Result<(), AmError> {
        match self.fault {
            Fault::Intent => Err(AmError::new(ErrorCode::NotFound, "no intent can exist")),
            Fault::Outcome => Err(AmError::new(ErrorCode::Internal, INJECTED_OUTCOME_FAILURE)),
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

pub(crate) fn event_rows(events: &dyn LocalEventStore, action_id: &str) -> Vec<LocalEventView> {
    events
        .list_events()
        .expect("read the events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect()
}

pub(crate) fn ctx() -> OpContext {
    OpContext::new(Bank::new(BANK), Actor::Harness)
}

pub(crate) fn accepted() -> BindingControls {
    BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    }
}

pub(crate) fn spec<'a>(id: &'a str, destination: &'a Path, staging: &'a Path) -> DeliverySpec<'a> {
    DeliverySpec {
        action_id: id,
        destination,
        staging_dir: staging,
        custody_owner: "clio",
        required_action: "delivery",
        now: NOW,
    }
}

pub(crate) fn restore_claim(journal: &Path) -> ArchiveClaim {
    match load_journal(journal) {
        JournalState::Current(state) => ArchiveClaim {
            deployment_id: state.deployment_id.clone(),
            generation: state.generation,
            salt_fingerprint: state.salt_fingerprint,
        },
        other => panic!("expected a current journal, got {other:?}"),
    }
}

pub(crate) fn restore_request<'a>(
    journal: &'a Path,
    claim: &'a ArchiveClaim,
) -> RestoreRequest<'a> {
    RestoreRequest {
        journal_path: journal,
        presented: None,
        claim,
        members: &[MemberFacts::None],
        actor: "harness",
        action_id: "restore-act",
        now: NOW,
    }
}

pub(crate) fn pg_or_skip() -> Option<(PgTestGuard, ScopedSchema, PostgresStore)> {
    let url = pg_url();
    if !require_postgres(postgres_reachable(&url)) {
        return None;
    }
    let guard = lock_pg();
    let schema = ScopedSchema::new(&url);
    let store = schema.open();
    Some((guard, schema, store))
}

pub(crate) fn started_erasure(request_id: &str) -> SubjectRequestRecord {
    let scope = RequestScope::Subject {
        bank_id: BANK.to_owned(),
        subject_id: DENIED_SUBJECT.to_owned(),
    };
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register(request_id, RequestKind::Erasure, &scope, &[])
        .expect("open the request");
    ledger
        .apply(request_id, &RequestCommand::Begin)
        .expect("begin the work");
    ledger
        .record(request_id)
        .expect("the record exists")
        .clone()
}

pub(crate) fn seeded_erasure_store(db: &Path, kms: &Path, request_id: &str) -> SqliteStore {
    let store = open_sqlite(db, kms);
    store
        .save(&record_to_row(&started_erasure(request_id)))
        .expect("persist the request");
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER);
    store
}

pub(crate) fn erase_args<'a>(
    store: &'a dyn ComplianceStore,
    ctx: &'a OpContext,
    sink: &'a EraseVecSink,
    journal: &'a Path,
    request_id: &'a str,
    salt: &'a [u8],
) -> EraseArgs<'a> {
    EraseArgs {
        store,
        journal_path: journal,
        maint: None,
        ctx,
        subject_id: DENIED_SUBJECT,
        legal_basis: "GDPR_ART_17",
        request_id,
        reason_code: None,
        confirm: true,
        salt,
        timestamp: NOW,
        sink,
    }
}

/// A failed export intent refuses before staging, the destination, or custody.
#[test]
fn an_export_intent_failure_refuses_before_any_effect() {
    let dir = scratch("export-intent");
    let store = open_sqlite(&dir.join("store.db"), &dir.join("kms"));
    let (destination, staging) = (dir.join("out.bin"), dir.join("staging"));
    let events = FaultStore {
        inner: &store,
        fault: Fault::Intent,
    };
    let result = deliver_export(
        &ctx(),
        &events,
        &store,
        &accepted(),
        &spec("export-act", &destination, &staging),
        b"payload",
    );
    assert_eq!(
        result,
        DeliveryResult::Refused(DeliveryRefusal::DurableIntentFailed)
    );
    assert!(
        !destination.exists() && !staging.exists(),
        "a refused intent writes nothing"
    );
    let custody = DestinationCustodyStore::load(&store, "export-act").expect("load");
    assert!(custody.is_none(), "no custody row");
    assert!(event_rows(&store, "export-act").is_empty(), "no event row");
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
    let live = store
        .erase_bank_set(BANK, DENIED_SUBJECT)
        .expect("live banks");
    let events = FaultStore {
        inner: &store,
        fault: Fault::Intent,
    };
    let err = erase_confirmed_request(&store, &events, &store, request_id, &args, &live)
        .expect_err("the intent fault refuses");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        store.list_events().expect("events").is_empty(),
        "no action row"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the queue empties"
    );
    assert_eq!(
        store
            .read_tombstones(BANK, request_id)
            .expect("tombstones")
            .len(),
        0,
        "no effect"
    );
    assert!(
        !store.subject_dek_destroyed(DENIED_SUBJECT).expect("key"),
        "no key loss"
    );
    assert!(!journal.exists(), "no journal denial is recorded");
}

/// A failed restore intent refuses before the caller can place anything.
#[test]
fn a_restore_intent_failure_refuses_before_placement() {
    let dir = scratch("restore-intent");
    let journal = dir.join("journal.json");
    record_denial(&journal, "denied-elsewhere", &[]).expect("seed the journal");
    let claim = restore_claim(&journal);
    let store = open_sqlite(&dir.join("events.db"), &dir.join("kms"));
    let events = FaultStore {
        inner: &store,
        fault: Fault::Intent,
    };
    let Err(refusal) = begin_restore(&events, &restore_request(&journal, &claim)) else {
        panic!("the intent fault must refuse");
    };
    assert!(matches!(refusal, RestoreGuardRefusal::Event(_)));
    assert!(
        !dir.join("target.db").exists(),
        "no guard means no placement can happen"
    );
}
