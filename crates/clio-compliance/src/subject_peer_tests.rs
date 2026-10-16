// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-peer request lifecycle coverage on
//! both backends: the stable action key, the idempotent snapshot and its
//! restart survival, the durable acknowledgement requirement, the disposition
//! record, the not-applicable round trip, and the slot fold.
//!
//! ## Owns
//! - The snapshot/restart/acknowledgement/fold assertions
//!
//! ## Does not own
//! - The lifecycle helpers (`subject_peer.rs`) and the row store
//!   (`clio-store`'s `peer_actions`)
//! - The disposition evidence coverage (`subject_peer_disposition_tests.rs`)
//! - The scratch SQLite fixture (`subject_peer_fixtures.rs`)
//! - The frozen record vocabulary (`subject_request_frozen_tests.rs`) and the
//!   register binding (`subject_request_register_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the key derivation, the snapshot rules, or the fold
//! change. The completion gate over the folded slots belongs to its own suite.
//!
//! Test-only. Never compiled into production builds.

use super::subject_peer_fixtures::{Scratch, kms, open_sqlite};
use crate::inventory_pg_support_tests::{pg_checked_url, pg_skip_note};
use crate::subject_peer::{
    derived_peer_states, mark_acknowledged, mark_delivered, mark_refused, peer_action_id,
    record_disposition, snapshot_peers,
};
use crate::subject_request::{
    PeerEnablement, PeerState, RequestKind, RequestScope, SubjectRequestLedger,
    SubjectRequestRecord,
};
use crate::subject_request_store_wiring::{record_to_row, row_to_record};
use crate::test_unique::unique;
use clio_store::{
    DurableRequestStore, PeerActionDisposition, PeerActionKind, PeerActionRow, PeerActionState,
    PeerActionStore, PgTestGuard, PostgresStore, ScopedSchema, lock_pg_test, probe_postgres,
};
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeMap;

/// The stored record of a freshly opened request. The fixture drives the peer
/// lifecycle only, so the frozen peer-bank set stays empty.
pub(super) fn open_record(
    request_id: &str,
    enablement: PeerEnablement,
    peers: &[&str],
) -> SubjectRequestRecord {
    let mut ledger = SubjectRequestLedger::new();
    let scope = RequestScope::Subject {
        bank_id: unique("peer-bank"),
        subject_id: unique("peer-subject"),
    };
    ledger
        .register_with_enablement(
            request_id,
            RequestKind::Erasure,
            &scope,
            enablement,
            peers,
            &[],
        )
        .expect("the request opens");
    ledger
        .record(request_id)
        .expect("the record exists")
        .clone()
}

/// One durable row as the store would hold it under the peer's derived key.
fn row(
    request_id: &str,
    peer_id: &str,
    state: PeerActionState,
    disposition: PeerActionDisposition,
    proof_ref: Option<&str>,
) -> PeerActionRow {
    PeerActionRow {
        request_id: request_id.into(),
        peer_id: peer_id.into(),
        action_kind: PeerActionKind::Denial,
        action_id: peer_action_id(request_id, peer_id),
        state,
        disposition,
        proof_ref: proof_ref.map(str::to_owned),
    }
}

/// The slots the restart tests fold: peer-a acknowledged, peer-b disposed.
fn restart_slots() -> BTreeMap<String, PeerState> {
    BTreeMap::from([
        ("peer-a".to_owned(), PeerState::Acknowledged),
        ("peer-b".to_owned(), PeerState::Disposed),
    ])
}

/// Open the live Postgres fixture over a private schema; an unreachable
/// Postgres panics unless the operator opted out with `CLIO_SKIP_PG_TESTS`.
fn pg_fixture_or_skip() -> Option<(PgTestGuard, ScopedSchema, PostgresStore)> {
    let guard = lock_pg_test();
    let base = pg_checked_url();
    if let Err(detail) = probe_postgres(&base) {
        let err = AmError::new(ErrorCode::Internal, format!("postgres probe: {detail}"));
        if matches!(
            std::env::var("CLIO_SKIP_PG_TESTS").as_deref(),
            Ok("1" | "true" | "yes")
        ) {
            eprintln!("{}", pg_skip_note("subject peer Postgres tests", &err));
            return None;
        }
        panic!("subject peer Postgres tests require a reachable Postgres: {err}");
    }
    let schema = ScopedSchema::new(&base);
    let store = PostgresStore::open(schema.url(), kms()).expect("open the scoped fixture store");
    Some((guard, schema, store))
}

/// The restart boundary: reopen the same private schema.
fn reopen_pg(schema: &ScopedSchema) -> PostgresStore {
    PostgresStore::open(schema.url(), kms()).expect("reopen the fixture store on the same schema")
}

/// The key derives from the request and peer in the crate's erasure scheme.
#[test]
fn peer_action_ids_are_stable_and_distinct_per_request_and_peer() {
    let one = peer_action_id("req-1", "peer-a");
    assert_eq!(one, peer_action_id("req-1", "peer-a"), "the key is stable");
    assert_ne!(one, peer_action_id("req-1", "peer-b"), "each peer differs");
    assert_ne!(
        one,
        peer_action_id("req-2", "peer-a"),
        "each request differs"
    );
    assert_eq!(one, "erasure:req-1:peer-a", "the derivation is pinned");
}

/// A snapshot writes one pending row per peer and a duplicate changes nothing.
#[test]
fn a_snapshot_writes_one_pending_row_per_enabled_peer_and_a_duplicate_is_a_no_op() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let record = open_record(
        "req-1",
        PeerEnablement::Required,
        &["peer-b", "peer-a", "peer-b"],
    );
    let rows = snapshot_peers(&store, &record).expect("the snapshot writes");
    assert_eq!(rows.len(), 2, "duplicates collapse to one row per peer");
    assert!(rows.iter().all(|row| row.state == PeerActionState::Pending));
    assert!(
        rows.iter()
            .all(|row| row.disposition == PeerActionDisposition::Unknown)
    );
    assert!(rows.iter().all(|row| row.proof_ref.is_none()));
    let again = snapshot_peers(&store, &record).expect("the duplicate snapshot");
    assert_eq!(again, rows, "a duplicate snapshot returns the stored rows");
    let delivered = mark_delivered(&store, "req-1", "peer-a").expect("deliver");
    assert_eq!(
        delivered.state,
        PeerActionState::Delivered,
        "the delivery is durable"
    );
    let retried = snapshot_peers(&store, &record).expect("the retry after delivery");
    assert_eq!(
        retried,
        vec![delivered, rows[1].clone()],
        "a retry cannot add a row or regress a settled one"
    );
    let other = open_record("req-2", PeerEnablement::Required, &["peer-z"]);
    assert_eq!(
        snapshot_peers(&store, &other)
            .expect("the other snapshot")
            .len(),
        1
    );
    assert_eq!(
        store.load_for_request("req-1").expect("list"),
        retried,
        "unrelated requests are untouched"
    );
}

/// The rows restart verbatim and the fold reproduces the same slots.
#[test]
fn the_snapshot_survives_a_reopen_and_the_fold_reproduces_it() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let record = open_record("req-1", PeerEnablement::Required, &["peer-a", "peer-b"]);
    let (acked, disposed) = {
        let store = open_sqlite(&path);
        snapshot_peers(&store, &record).expect("the snapshot");
        let acked =
            mark_acknowledged(&store, "req-1", "peer-a", "generation=1").expect("acknowledge");
        let removal = PeerActionDisposition::RemovalEvidence;
        let disposed = record_disposition(&store, "req-1", "peer-b", removal, Some("deletion-9"))
            .expect("dispose");
        assert_eq!(
            derived_peer_states(&record, &store.load_for_request("req-1").expect("list")),
            restart_slots(),
            "acknowledged and disposed derive from the durable rows"
        );
        (acked, disposed)
    };
    let reopened = open_sqlite(&path);
    let rows = reopened
        .load_for_request("req-1")
        .expect("the restart read");
    assert_eq!(rows, vec![acked, disposed], "the rows restart verbatim");
    assert_eq!(
        snapshot_peers(&reopened, &record).expect("the restart retry"),
        rows,
        "a restart retry adds no row and repeats no settle"
    );
    assert_eq!(
        derived_peer_states(&record, &rows),
        restart_slots(),
        "the same slots derive after a restart"
    );
}

/// An acknowledgement is refused when the durable row does not exist.
#[test]
fn an_acknowledgement_requires_the_durable_row() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let err = mark_acknowledged(&store, "req-1", "peer-1", "generation=1")
        .expect_err("no durable row refuses");
    assert_eq!(err.code, ErrorCode::NotFound);
    let record = open_record("req-1", PeerEnablement::Required, &["peer-1"]);
    snapshot_peers(&store, &record).expect("the snapshot");
    let acked = mark_acknowledged(&store, "req-1", "peer-1", "generation=1").expect("acknowledge");
    assert_eq!(acked.state, PeerActionState::Acknowledged);
    assert_eq!(acked.disposition, PeerActionDisposition::RemovalEvidence);
    assert_eq!(acked.proof_ref.as_deref(), Some("generation=1"));
    let duplicate =
        mark_acknowledged(&store, "req-1", "peer-1", "generation=2").expect("duplicate ack");
    assert_eq!(duplicate, acked, "the first durable proof stands");
}

/// A bare refusal records no disposition, so the peer stays open.
#[test]
fn a_refusal_without_a_disposition_leaves_the_peer_open() {
    let scratch = Scratch::new();
    let store = open_sqlite(&scratch.0.join("store.db"));
    let record = open_record("req-1", PeerEnablement::Required, &["peer-1"]);
    snapshot_peers(&store, &record).expect("the snapshot");
    let refused = mark_refused(&store, "req-1", "peer-1").expect("refuse");
    assert_eq!(refused.state, PeerActionState::Refused);
    assert_eq!(refused.disposition, PeerActionDisposition::Unknown);
    let rows = store.load_for_request("req-1").expect("list");
    assert_eq!(
        derived_peer_states(&record, &rows)["peer-1"],
        PeerState::Pending,
        "no disposition means no disposal"
    );
    let again = mark_refused(&store, "req-1", "peer-1").expect("the duplicate refusal");
    assert_eq!(again, refused, "the refusal is recorded once");
    assert_eq!(
        store.load_for_request("req-1").expect("list"),
        rows,
        "a duplicate refusal changes nothing"
    );
}

/// The fold reads only rows bound to the peer's own derived key.
#[test]
fn the_fold_derives_slots_only_from_rows_bound_to_the_peers_own_key() {
    use clio_store::PeerActionDisposition as D;
    use clio_store::PeerActionState as S;
    let record = open_record(
        "req-1",
        PeerEnablement::Required,
        &["peer-a", "peer-b", "peer-c", "peer-d"],
    );
    let rows = vec![
        row(
            "req-1",
            "peer-a",
            S::Acknowledged,
            D::RemovalEvidence,
            Some("generation=1"),
        ),
        row(
            "req-1",
            "peer-b",
            S::Refused,
            D::RetentionApproved,
            Some("hold-7"),
        ),
        row("req-1", "peer-c", S::Refused, D::Unknown, None),
        row("req-1", "peer-d", S::Delivered, D::Unknown, None),
    ];
    assert_eq!(
        derived_peer_states(&record, &rows),
        BTreeMap::from([
            ("peer-a".to_owned(), PeerState::Acknowledged),
            ("peer-b".to_owned(), PeerState::Pending),
            ("peer-c".to_owned(), PeerState::Pending),
            ("peer-d".to_owned(), PeerState::Pending),
        ]),
        "an approval or a refusal without removal evidence keeps the peer open"
    );
    let mut mismatched = rows[0].clone();
    mismatched.action_id = "other:key".into();
    let mut foreign = rows[1].clone();
    foreign.request_id = "req-2".into();
    assert!(
        derived_peer_states(&record, &[mismatched, foreign])
            .values()
            .all(|slot| *slot == PeerState::Pending),
        "a mismatched key or another request never moves a slot"
    );
}

/// A peers-disabled request records not applicable and round-trips durably.
#[test]
fn a_peers_disabled_request_records_not_applicable_and_snapshots_nothing() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let record = open_record("req-1", PeerEnablement::NotApplicable, &[]);
    assert_eq!(record.peer_enablement, PeerEnablement::NotApplicable);
    assert!(record.peers.is_empty(), "no enabled peer records exist");
    {
        let store = open_sqlite(&path);
        let no_rows = snapshot_peers(&store, &record).expect("no peer work");
        assert_eq!(
            no_rows,
            Vec::new(),
            "a peers-disabled request snapshots nothing"
        );
        assert_eq!(store.load_for_request("req-1").expect("list"), Vec::new());
        store
            .save(&record_to_row(&record))
            .expect("the record persists");
    }
    let reopened = open_sqlite(&path);
    let reloaded = row_to_record(
        &DurableRequestStore::load(&reopened, "req-1")
            .expect("load")
            .expect("the row exists"),
    )
    .expect("decode");
    assert_eq!(reloaded, record, "the record round-trips");
    assert_eq!(
        reloaded.peer_enablement,
        PeerEnablement::NotApplicable,
        "a known not-applicable fact, never unknown"
    );
}

/// The Postgres snapshot and the settles restart verbatim.
#[test]
fn postgres_the_snapshot_and_the_settles_restart_verbatim() {
    let Some((_guard, schema, store)) = pg_fixture_or_skip() else {
        return;
    };
    let record = open_record(
        &unique("pg-peer-req"),
        PeerEnablement::Required,
        &["peer-a", "peer-b"],
    );
    let request_id = record.request_id.clone();
    let err = mark_acknowledged(&store, &request_id, "peer-a", "generation=1")
        .expect_err("no durable row refuses");
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(
        snapshot_peers(&store, &record).expect("the snapshot").len(),
        2,
        "one row per enabled peer"
    );
    let acked = mark_acknowledged(&store, &request_id, "peer-a", "generation=1").expect("ack");
    let removal = PeerActionDisposition::RemovalEvidence;
    let disposed = record_disposition(&store, &request_id, "peer-b", removal, Some("deletion-9"))
        .expect("dispose");
    let repeated = snapshot_peers(&store, &record).expect("the duplicate snapshot");
    assert_eq!(repeated.len(), 2, "a duplicate snapshot adds no row");
    let reopened = reopen_pg(&schema);
    let rows = reopened
        .load_for_request(&request_id)
        .expect("the restart read");
    assert_eq!(rows, vec![acked, disposed], "the rows restart verbatim");
    assert_eq!(
        snapshot_peers(&reopened, &record).expect("the restart retry"),
        rows,
        "a restart retry repeats nothing"
    );
    assert_eq!(
        derived_peer_states(&record, &rows),
        restart_slots(),
        "the same slots derive after a restart"
    );
}

/// A peers-disabled record round-trips on Postgres and snapshots nothing.
#[test]
fn postgres_a_peers_disabled_request_round_trips_and_snapshots_nothing() {
    let Some((_guard, schema, store)) = pg_fixture_or_skip() else {
        return;
    };
    let record = open_record(&unique("pg-peer-req"), PeerEnablement::NotApplicable, &[]);
    store
        .save(&record_to_row(&record))
        .expect("the record persists");
    let reopened = reopen_pg(&schema);
    let reloaded = row_to_record(
        &DurableRequestStore::load(&reopened, &record.request_id)
            .expect("load")
            .expect("the row exists"),
    )
    .expect("decode");
    assert_eq!(reloaded, record, "the record round-trips");
    assert_eq!(reloaded.peer_enablement, PeerEnablement::NotApplicable);
    let no_rows = snapshot_peers(&reopened, &reloaded).expect("no peer work");
    assert_eq!(
        no_rows,
        Vec::new(),
        "a peers-disabled request snapshots nothing"
    );
}
