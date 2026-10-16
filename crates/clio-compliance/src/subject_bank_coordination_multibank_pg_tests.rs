// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres multi-bank coordination
//! proofs: a retry after a failure that lands past the key destruction but
//! before the commit still purges the remaining work over the banks that share
//! the subject key, keeps the control subject readable, and holds a pending
//! peer until it acknowledges; and a real `MemTreeMaint` drops the erased
//! subject's gist from the shared derived summary once the retry refresh runs.
//!
//! ## Owns
//! - The Postgres multi-bank key-loss retry assertion
//! - The Postgres shared-derivative summary assertion
//!
//! ## Does not own
//! - The coordination refusal (`subject_bank_coordination.rs`)
//! - The Postgres coordination refusals and their fixture
//!   (`subject_bank_coordination_pg_tests.rs`)
//! - The retry contract (`erasure_retry.rs`) and the reconcile
//!   (`erasure_reconcile.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres retry stops purging the remaining work
//! past a key loss or the derived refresh stops dropping the erased gist.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the Postgres multi-bank key-loss retry and
//! shared-derivative proofs.

use super::{
    BANK_A, BANK_B, PgStore, SUBJECT, confirmed, erase_args, live_bank_set, pg_store_or_skip,
    subject_item, two_bank,
};
use crate::erase::{EraseArgs, EraseVecSink};
use crate::erasure_reconcile::{ErasureResume, ErasureRetryEnv, reconcile_erasure_actions};
use crate::erasure_retry::{OUTCOME_COMPLETED, erase_confirmed_request, erasure_action_id};
use crate::export_subject_scenarios_tests::FaultEvents;
use crate::inventory_fixture::FixtureDir;
use crate::subject_request::{PeerState, RequestKind, RequestScope, SubjectRequestRecord};
use crate::subject_request_store_wiring::{record_to_row, row_to_record};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, DurableRequestStore, LocalEventState, LocalEventStore, PostgresStore, Store,
};
use clio_types::{Actor, Bank, ErrorCode, OpContext};
use clio_write::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use clio_write::{MemForest, MemTreeMaint};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Persist `record`'s current state to the Postgres store.
fn persist(store: &PostgresStore, record: &SubjectRequestRecord) {
    store.save(&record_to_row(record)).expect("the row saves");
}

/// The root node's aggregate summary for one bank, or the empty string.
fn root_summary(forest: &Arc<Mutex<MemForest>>, bank: &str) -> String {
    let forest = forest.lock().expect("forest");
    let tree = forest.tree(bank).expect("tree");
    tree.get(tree.root_id().expect("root"))
        .and_then(|node| node.summary_gist.clone())
        .unwrap_or_default()
}

/// Install a `BEFORE INSERT` trigger that aborts every tombstone write, so the
/// Postgres erasure fails after it destroys the key and before it commits.
fn install_tombstone_trigger(fixture: &PgStore) {
    fixture
        .execute(
            "CREATE FUNCTION fail_erase_tombstone() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'forced tombstone failure'; END $$",
        )
        .expect("create the fault function");
    fixture
        .execute(
            "CREATE TRIGGER fail_erase_tombstone BEFORE INSERT ON compliance_tombstones
             FOR EACH ROW EXECUTE FUNCTION fail_erase_tombstone()",
        )
        .expect("create the fault trigger");
}

/// Remove the injected tombstone trigger so a later retry can commit.
fn drop_tombstone_trigger(fixture: &PgStore) {
    fixture
        .execute("DROP TRIGGER IF EXISTS fail_erase_tombstone ON compliance_tombstones")
        .expect("drop the fault trigger");
    fixture
        .execute("DROP FUNCTION IF EXISTS fail_erase_tombstone()")
        .expect("drop the fault function");
}

/// A pending peer refuses each retry before any intent, key loss, or store
/// effect; the exact denial repeats and the stored peer stays pending.
fn assert_pending_peer_refuses_each_attempt(
    fixture: &PgStore,
    record: &SubjectRequestRecord,
    args: &EraseArgs<'_>,
    live: &[String],
) {
    for _ in 0..2 {
        let err = erase_confirmed_request(
            &fixture.store,
            &fixture.store,
            &record.request_id,
            args,
            live,
        )
        .expect_err("a pending peer must refuse");
        assert_eq!(err.code, ErrorCode::Forbidden, "the same denial repeats");
        assert!(
            !fixture.store.subject_dek_destroyed(SUBJECT).expect("key"),
            "the refusal loses no key"
        );
        assert_eq!(
            fixture
                .store
                .read_tombstones(BANK_A, &record.request_id)
                .expect("tombstones")
                .len(),
            0,
            "the refusal writes no tombstone"
        );
    }
    let stored = row_to_record(
        &fixture
            .store
            .load(&record.request_id)
            .expect("load")
            .expect("row exists"),
    )
    .expect("decode");
    assert_eq!(
        stored.peers.get(BANK_B),
        Some(&PeerState::Pending),
        "the peer stays pending until it acknowledges"
    );
}

/// Force a failure after the DEK destruction and before the commit; the key is
/// lost, no tombstone is written, and the action stays pending.
fn force_key_loss_before_commit(
    fixture: &PgStore,
    record: &SubjectRequestRecord,
    args: &EraseArgs<'_>,
    live: &[String],
) {
    install_tombstone_trigger(fixture);
    let failed = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &record.request_id,
        args,
        live,
    )
    .expect("the key-loss attempt reports processing");
    assert_eq!(
        failed.status, "PROCESSING",
        "the failed purge stays pending past the key loss"
    );
    assert!(
        fixture.store.subject_dek_destroyed(SUBJECT).expect("key"),
        "the key is destroyed before the commit fails"
    );
    assert_eq!(
        fixture
            .store
            .read_tombstones(BANK_A, &record.request_id)
            .expect("tombstones")
            .len(),
        0,
        "the aborted purge wrote no tombstone"
    );
    assert_eq!(
        fixture.store.pending_actions().expect("pending"),
        vec![erasure_action_id(&record.request_id)],
        "the action stays pending"
    );
}

/// Remove the injected trigger and retry: the purge commits, the control
/// subject stays readable, the scope is unchanged, and one Allowed event row
/// carries the completed outcome.
fn assert_retry_purges_remaining_work(
    fixture: &PgStore,
    record: &SubjectRequestRecord,
    control_id: &str,
    args: &EraseArgs<'_>,
    live: &[String],
) {
    drop_tombstone_trigger(fixture);
    let completed = erase_confirmed_request(
        &fixture.store,
        &fixture.store,
        &record.request_id,
        args,
        live,
    )
    .expect("the retry purges the remaining work");
    assert_eq!(completed.status, "COMPLETED");
    assert!(
        !fixture
            .store
            .read_tombstones(BANK_A, &record.request_id)
            .expect("tombstones")
            .is_empty(),
        "the retry commits the purge"
    );
    assert!(
        fixture
            .store
            .get_memory_item(BANK_A, control_id)
            .expect("readable")
            .is_some(),
        "the control subject stays readable"
    );
    assert_eq!(
        live_bank_set(&fixture.store, BANK_A, SUBJECT),
        live,
        "the retry preserves the exact scope"
    );
    let action_id = erasure_action_id(&record.request_id);
    let rows: Vec<_> = fixture
        .store
        .list_events()
        .expect("events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect();
    assert_eq!(rows.len(), 1, "one event row for the action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert_eq!(rows[0].result.as_deref(), Some(OUTCOME_COMPLETED));
}

/// A failure after the Postgres key destruction but before the commit leaves
/// the retry to purge the remaining work over two banks that share the subject
/// key, keeps the control subject readable, and holds a pending peer until it
/// acknowledges.
#[test]
fn postgres_multi_bank_key_loss_retry_purges_remaining_work_and_holds_the_peer_pending() {
    let Some(fixture) = pg_store_or_skip() else {
        return;
    };
    let mut record = two_bank(PeerState::Pending);
    let control_subject = unique("pg-control-subject");
    let item_a = subject_item(BANK_A, SUBJECT);
    let item_b = subject_item(BANK_B, SUBJECT);
    let control = subject_item(BANK_A, &control_subject);
    fixture
        .store
        .create_memory_item(&item_a, "tester")
        .expect("seed the subject item in bank a");
    fixture
        .store
        .create_memory_item(&item_b, "tester")
        .expect("seed the shared subject item in bank b");
    fixture
        .store
        .create_memory_item(&control, "tester")
        .expect("seed the control subject item");
    persist(&fixture.store, &record);
    let live = live_bank_set(&fixture.store, BANK_A, SUBJECT);
    assert_eq!(
        live,
        vec![BANK_A.to_owned(), BANK_B.to_owned()],
        "both banks share the subject key: {live:?}"
    );
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let dir = FixtureDir::create(&unique("pg-coord-multibank-journal"));
    let journal = dir.path().join("erasure-journal.json");
    let args = erase_args(
        &fixture.store,
        &journal,
        &ctx,
        &sink,
        &record.request_id,
        SUBJECT,
        &salt,
    );
    // A pending peer refuses each attempt before any intent, key loss, or
    // store effect, and the exact denial repeats.
    assert_pending_peer_refuses_each_attempt(&fixture, &record, &args, &live);
    // Acknowledge the peer, then force a failure after the DEK destruction and
    // before the commit.
    record
        .peers
        .insert(BANK_B.to_owned(), PeerState::Acknowledged);
    persist(&fixture.store, &record);
    force_key_loss_before_commit(&fixture, &record, &args, &live);
    // The retry purges the remaining work and settles, keeping the control
    // subject readable and the scope unchanged.
    assert_retry_purges_remaining_work(&fixture, &record, &control.id, &args, &live);
}

/// A real `MemTreeMaint` no longer exposes the erased gist in the shared
/// derived summary once the Postgres retry refresh runs.
#[test]
fn postgres_shared_derivative_summary_drops_the_erased_gist_after_the_retry() {
    let Some(fixture) = pg_store_or_skip() else {
        return;
    };
    let record = confirmed(
        RequestKind::Erasure,
        RequestScope::Subject {
            bank_id: BANK_A.into(),
            subject_id: SUBJECT.into(),
        },
        BTreeMap::new(),
    );
    let item = subject_item(BANK_A, SUBJECT);
    let item_id = item.id.clone();
    fixture
        .store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    persist(&fixture.store, &record);
    let live = live_bank_set(&fixture.store, BANK_A, SUBJECT);
    assert_eq!(live, vec![BANK_A.to_owned()], "one bank holds the key");
    // The live derived state still holds the erased leaf, as it does when the
    // process died before the refresh.
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    leaves.publish(item);
    maint.on_leaf_attached(MaintenanceSignal::leaf_attached(&item_id, BANK_A));
    assert!(!maint.refresh_bank(BANK_A).is_empty(), "the seed refreshes");
    let before = root_summary(&forest, BANK_A);
    assert!(before.contains("secret gist"), "{before}");
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let dir = FixtureDir::create(&unique("pg-coord-derivative-journal"));
    let journal = dir.path().join("erasure-journal.json");
    let args = erase_args(
        &fixture.store,
        &journal,
        &ctx,
        &sink,
        &record.request_id,
        SUBJECT,
        &salt,
    );
    // The effect commits but the outcome write fails, so a restart reconciles
    // and still owes the derived refresh.
    let events = FaultEvents::outcome_fails_over(&fixture.store);
    erase_confirmed_request(&fixture.store, &events, &record.request_id, &args, &live)
        .expect_err("the outcome failure leaves the action pending");
    assert!(
        fixture.store.subject_dek_destroyed(SUBJECT).expect("key"),
        "the effect destroys the key"
    );
    assert!(
        leaves.get(BANK_A, &item_id).is_some(),
        "the leaf stays until the retry refresh"
    );
    let env = ErasureRetryEnv {
        store: &fixture.store,
        journal_path: &journal,
        maint: Some(&maint),
        actor: Actor::Harness,
        legal_basis: "GDPR_ART_17",
        salt: &salt,
        timestamp: "2026-01-01T00:00:00Z",
        sink: &sink,
    };
    let resume = |_: &SubjectRequestRecord| {
        Some(ErasureResume {
            subject_id: SUBJECT.to_owned(),
            live_banks: live.clone(),
        })
    };
    let report = reconcile_erasure_actions(&fixture.store, &fixture.store, &env, &resume)
        .expect("reconcile");
    assert_eq!(report.settled, vec![erasure_action_id(&record.request_id)]);
    assert!(
        leaves.get(BANK_A, &item_id).is_none(),
        "the erased leaf is detached"
    );
    let after = root_summary(&forest, BANK_A);
    assert!(!after.contains("secret gist"), "{after}");
}
