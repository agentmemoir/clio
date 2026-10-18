// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the outcome-failure arm of each export,
//! erasure, review-export, restore, and Postgres export route, and for the
//! restart-and-reconcile assertions that settle the pending action without
//! repeating an irreversible effect.
//!
//! ## Owns
//! - The outcome-failure and restart-reconcile arm for each route
//!
//! ## Does not own
//! - The shared `Fault`/`FaultStore` doubles and the intent-failure arms (see
//!   `scenario_intent_outcome_faults`)
//! - The seeded stores and read-back probes (see `fixtures` and `support`)
//!
//! ## Boundary
//!
//! Changes to which routes or fault points this claim proves should normally
//! require changes here. Changes to the durable store contract should not.

use crate::cli::salt_bytes;
use crate::fixtures::{
    BANK, DENIED_ITEM, DENIED_MARKER, DENIED_SUBJECT, NOW, backup_dir, open_sqlite, scratch,
    seed_sqlite,
};
use crate::scenario_intent_outcome_faults::{
    Fault, FaultStore, accepted, ctx, erase_args, event_rows, pg_or_skip, restore_claim,
    restore_request, seeded_erasure_store, spec,
};
use crate::support::{Readback, archive_sqlite, readback, restore_sqlite};
use clio::store::{ComplianceStore, LocalEventState, LocalEventStore};
use clio::{Actor, Bank, ErrorCode, OpContext};
use clio_compliance::{
    DeliveryRefusal, DeliveryResult, EraseVecSink, ErasureResume, ErasureRetryEnv,
    LocalReviewQuery, SubjectRequestRecord, begin_restore, deliver_export, erase_confirmed_request,
    reconcile_erasure_actions, reconcile_open_destinations, reconcile_restore_actions,
    record_denial, write_review_export,
};
use std::path::Path;

/// A failed export outcome leaves one effect and one pending action; a restart
/// reconciles it, and a repeat before reconcile never adds a second effect.
#[test]
fn an_export_outcome_failure_reconciles_without_a_second_effect() {
    let dir = scratch("export-outcome");
    let db = dir.join("store.db");
    let (destination, staging) = (dir.join("out.bin"), dir.join("staging"));
    {
        let store = open_sqlite(&db, &dir.join("kms"));
        let events = FaultStore {
            inner: &store,
            fault: Fault::Outcome,
        };
        let result = deliver_export(
            &ctx(),
            &events,
            &store,
            &accepted(),
            &spec("export-act", &destination, &staging),
            b"payload",
        );
        assert!(
            matches!(&result, DeliveryResult::Failed(reason) if reason.contains("not settled")),
            "the outcome fault is reported: {result:?}"
        );
        assert_eq!(
            event_rows(&store, "export-act")[0].state,
            LocalEventState::Pending
        );
        assert_eq!(
            store.pending_actions().expect("pending"),
            vec!["export-act".to_owned()]
        );
        let repeat = deliver_export(
            &ctx(),
            &events,
            &store,
            &accepted(),
            &spec("export-act", &destination, &staging),
            b"second",
        );
        assert_eq!(
            repeat,
            DeliveryResult::Refused(DeliveryRefusal::DurableIntentFailed)
        );
        assert_eq!(
            std::fs::read(&destination).expect("destination"),
            b"payload",
            "no second effect"
        );
    }
    let store = open_sqlite(&db, &dir.join("kms"));
    assert!(
        reconcile_open_destinations(&store, &store)
            .expect("reconcile")
            .is_empty(),
        "the delivered row is not an open destination"
    );
    let rows = event_rows(&store, "export-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the queue empties"
    );
    assert_eq!(
        std::fs::read(&destination).expect("destination"),
        b"payload"
    );
}

/// A failed review-export outcome leaves one file effect and one pending
/// action that reconciles after a restart without redelivering.
#[test]
fn a_review_export_outcome_failure_reconciles_without_redelivery() {
    let dir = scratch("review-outcome");
    let db = dir.join("store.db");
    let (destination, staging) = (dir.join("review.txt"), dir.join("staging"));
    let query = LocalReviewQuery {
        limit: 100,
        cursor: None,
        request_id: None,
    };
    {
        let store = open_sqlite(&db, &dir.join("kms"));
        let events = FaultStore {
            inner: &store,
            fault: Fault::Outcome,
        };
        let err = write_review_export(
            &ctx(),
            &events,
            &store,
            &query,
            &accepted(),
            &spec("review-act", &destination, &staging),
        )
        .expect_err("the outcome fault is reported");
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(destination.exists(), "the file effect landed once");
        assert_eq!(
            event_rows(&store, "review-act")[0].state,
            LocalEventState::Pending
        );
        assert_eq!(
            store.pending_actions().expect("pending"),
            vec!["review-act".to_owned()]
        );
    }
    let store = open_sqlite(&db, &dir.join("kms"));
    reconcile_open_destinations(&store, &store).expect("reconcile");
    let rows = event_rows(&store, "review-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the queue empties"
    );
    assert!(
        destination.exists(),
        "the file is never redelivered or removed"
    );
}

/// Drive one erasure whose outcome write fails: it commits exactly one effect
/// and leaves one pending intent, and a repeat adds no second store effect.
/// Returns the pending action id and the committed tombstone count.
fn drive_failed_erasure_outcome(
    db: &Path,
    kms: &Path,
    journal: &Path,
    request_id: &str,
    salt: &[u8],
) -> (String, usize) {
    let store = seeded_erasure_store(db, kms, request_id);
    let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&store, &ctx, &sink, journal, request_id, salt);
    let live = store
        .erase_bank_set(BANK, DENIED_SUBJECT)
        .expect("live banks");
    let events = FaultStore {
        inner: &store,
        fault: Fault::Outcome,
    };
    erase_confirmed_request(&store, &events, &store, request_id, &args, &live)
        .expect_err("the outcome fault is reported");
    let action = store
        .pending_actions()
        .expect("pending")
        .pop()
        .expect("one pending action");
    let rows = event_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one intent row");
    assert_eq!(
        rows[0].state,
        LocalEventState::Pending,
        "the outcome is unwritten"
    );
    let tombstones = store
        .read_tombstones(BANK, request_id)
        .expect("tombstones")
        .len();
    assert!(tombstones > 0, "the effect committed");
    assert!(
        store.subject_dek_destroyed(DENIED_SUBJECT).expect("key"),
        "the key was destroyed"
    );
    let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
    let sink = EraseVecSink::default();
    let args = erase_args(&store, &ctx, &sink, journal, request_id, salt);
    let live = store
        .erase_bank_set(BANK, DENIED_SUBJECT)
        .expect("live banks");
    erase_confirmed_request(&store, &events, &store, request_id, &args, &live)
        .expect_err("the repeat cannot settle");
    assert_eq!(
        store
            .read_tombstones(BANK, request_id)
            .expect("tombstones")
            .len(),
        tombstones,
        "the repeat repeats no store effect"
    );
    (action, tombstones)
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
    let (action, tombstones) =
        drive_failed_erasure_outcome(&db, &dir.join("kms"), &journal, request_id, &salt);
    let store = open_sqlite(&db, &dir.join("kms"));
    let sink = EraseVecSink::default();
    let live = store
        .erase_bank_set(BANK, DENIED_SUBJECT)
        .expect("live banks");
    let env = ErasureRetryEnv {
        store: &store,
        peers: &store,
        journal_path: &journal,
        maint: None,
        actor: Actor::Harness,
        legal_basis: "GDPR_ART_17",
        salt: &salt,
        timestamp: NOW,
        sink: &sink,
    };
    let resume = |_: &SubjectRequestRecord| {
        Some(ErasureResume {
            subject_id: DENIED_SUBJECT.to_owned(),
            live_banks: live.clone(),
        })
    };
    let report = reconcile_erasure_actions(&store, &store, &env, &resume).expect("reconcile");
    assert_eq!(
        report.settled,
        vec![action.clone()],
        "the pending action settles"
    );
    assert!(report.pending.is_empty(), "{report:?}");
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the queue empties"
    );
    let rows = event_rows(&store, &action);
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(
        store
            .read_tombstones(BANK, request_id)
            .expect("tombstones")
            .len(),
        tombstones,
        "the restart repeats no store effect"
    );
}

/// A failed restore outcome leaves one placement and one pending action; the
/// restart reconcile denies it without repeating the placement.
#[test]
fn a_restore_outcome_failure_reconciles_without_repeating_placement() {
    let dir = scratch("restore-outcome");
    let (src_db, src_kms) = (dir.join("src.db"), dir.join("src.kms"));
    seed_sqlite(
        &src_db,
        &src_kms,
        &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)],
    );
    let archive = archive_sqlite(&backup_dir(&dir), &src_db, &src_kms, "restore-outcome");
    let journal = dir.join("journal.json");
    record_denial(&journal, "denied-elsewhere", &[]).expect("seed the journal");
    let claim = restore_claim(&journal);
    let (db, target_db, target_kms) = (
        dir.join("events.db"),
        dir.join("target.db"),
        dir.join("target.kms"),
    );
    {
        let store = open_sqlite(&db, &dir.join("kms"));
        let events = FaultStore {
            inner: &store,
            fault: Fault::Outcome,
        };
        let guard =
            begin_restore(&events, &restore_request(&journal, &claim)).expect("intent lands");
        // The effect the guard does not perform: the caller places the archive.
        restore_sqlite(&archive, &target_db, &target_kms, true);
        guard
            .settle(&events, true, "placed")
            .expect_err("the outcome write fails");
        let rows = event_rows(&store, "restore-act");
        assert_eq!(rows.len(), 1, "one intent row");
        assert_eq!(
            rows[0].state,
            LocalEventState::Pending,
            "the outcome is unwritten"
        );
    }
    assert_eq!(
        readback(&open_sqlite(&target_db, &target_kms), DENIED_ITEM),
        Readback::Readable,
        "the placement landed exactly once"
    );
    let store = open_sqlite(&db, &dir.join("kms"));
    assert_eq!(
        reconcile_restore_actions(&store).expect("reconcile"),
        vec!["restore-act".to_owned()],
        "the pending restore settles"
    );
    let rows = event_rows(&store, "restore-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Denied);
    assert!(
        rows[0].result.is_some(),
        "the reconcile names a content-free reason"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the queue empties"
    );
    let events = FaultStore {
        inner: &store,
        fault: Fault::Outcome,
    };
    assert!(
        begin_restore(&events, &restore_request(&journal, &claim)).is_err(),
        "the denied action never resumes"
    );
    assert_eq!(
        readback(&open_sqlite(&target_db, &target_kms), DENIED_ITEM),
        Readback::Readable,
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
        let events = FaultStore {
            inner: &store,
            fault: Fault::Outcome,
        };
        let result = deliver_export(
            &ctx(),
            &events,
            &store,
            &accepted(),
            &spec("pg-export-act", &destination, &staging),
            b"payload",
        );
        assert!(
            matches!(&result, DeliveryResult::Failed(reason) if reason.contains("not settled")),
            "the outcome fault is reported: {result:?}"
        );
    }
    assert_eq!(
        std::fs::read(&destination).expect("destination"),
        b"payload",
        "exactly one destination effect"
    );
    assert_eq!(
        event_rows(&store, "pg-export-act")[0].state,
        LocalEventState::Pending
    );
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec!["pg-export-act".to_owned()]
    );
    drop(store);
    let store = schema.open();
    assert!(
        reconcile_open_destinations(&store, &store)
            .expect("reconcile")
            .is_empty(),
        "the delivered row is not an open destination"
    );
    let rows = event_rows(&store, "pg-export-act");
    assert_eq!(rows.len(), 1, "one terminal outcome");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the queue empties"
    );
    assert_eq!(
        std::fs::read(&destination).expect("destination"),
        b"payload"
    );
}
