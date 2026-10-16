// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the subject-bank
//! coordination refusals: a pending or disposed peer and a changed bank
//! membership refuse before any erasure effect, and an acknowledged set runs
//! the real Postgres transaction and key store while a control subject stays
//! readable.
//!
//! ## Owns
//! - The Postgres coordination refusal and acknowledgement assertions
//! - Its own private-schema Postgres fixture
//!
//! ## Does not own
//! - The coordination check itself (`subject_bank_coordination.rs`)
//! - The shared SQLite coordination fixtures
//!   (`subject_bank_coordination_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres coordination stops refusing a changed
//! bank set before any effect.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::{EraseArgs, EraseVecSink};
use crate::inventory_fixture::FixtureDir;
use crate::inventory_pg_support_tests::{pg_checked_url, pg_skip_note};
use crate::subject_bank_coordination::coordinated_erase_request;
use crate::subject_request::{
    PeerState, RequestKind, RequestScope, RequestState, SubjectRequestRecord,
};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, DekProvider, LocalDevKms, PgTestGuard, PostgresStore, ScopedSchema, Store,
    lock_pg_test, probe_postgres,
};
use clio_types::{
    Actor, AmError, Bank, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext,
    SemanticCategory,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// The bank the confirmed request is bound to.
const BANK_A: &str = "bank-a";
/// The second bank that shares the subject key.
const BANK_B: &str = "bank-b";
/// The subject both banks hold.
const SUBJECT: &str = "subject-1";

/// A private-schema Postgres store, dropped before its schema.
struct PgStore {
    store: PostgresStore,
    schema: ScopedSchema,
    _guard: PgTestGuard,
}

impl PgStore {
    /// Run one batch of SQL against the fixture's private schema. The
    /// fault-injection a test needs (a trigger) has no store API to reach it,
    /// so the fixture's own schema connection carries it.
    fn execute(&self, sql: &str) -> Result<(), AmError> {
        self.schema.execute(sql)
    }
}

/// True only when the operator explicitly opted out of Postgres-backed tests.
/// Without this opt-out an unreachable Postgres fails the test instead of
/// silently reporting green.
fn skip_requested() -> bool {
    matches!(
        std::env::var("CLIO_SKIP_PG_TESTS").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

/// Open the live Postgres fixture. An unreachable Postgres fails the test
/// unless the operator set `CLIO_SKIP_PG_TESTS`, which prints the skip note and
/// returns `None`.
fn pg_store_or_skip() -> Option<PgStore> {
    let guard = lock_pg_test();
    let base = pg_checked_url();
    if let Err(detail) = probe_postgres(&base) {
        let err = AmError::new(ErrorCode::Internal, format!("postgres probe: {detail}"));
        if skip_requested() {
            eprintln!(
                "{}",
                pg_skip_note("subject bank coordination Postgres tests", &err)
            );
            return None;
        }
        panic!("subject bank coordination Postgres tests require Postgres: {err}");
    }
    let schema = ScopedSchema::new(&base);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(schema.url(), kms).expect("open the scoped fixture store");
    Some(PgStore {
        store,
        schema,
        _guard: guard,
    })
}

/// A confirmed erasure record with the given kind, scope, and peer slots.
fn confirmed(
    kind: RequestKind,
    scope: RequestScope,
    peers: BTreeMap<String, PeerState>,
) -> SubjectRequestRecord {
    SubjectRequestRecord {
        request_id: unique("pg-req"),
        kind,
        scope,
        state: RequestState::InProgress,
        verification: None,
        deadline: None,
        actions: BTreeMap::new(),
        paths: BTreeMap::new(),
        peers,
        exception: None,
    }
}

/// The two-bank scenario record: bank A's subject also lives in bank B.
fn two_bank(peer_b: PeerState) -> SubjectRequestRecord {
    let mut peers = BTreeMap::new();
    peers.insert(BANK_B.to_owned(), peer_b);
    confirmed(
        RequestKind::Erasure,
        RequestScope::Subject {
            bank_id: BANK_A.into(),
            subject_id: SUBJECT.into(),
        },
        peers,
    )
}

/// The live bank set the erasure effect fans out to for one subject, read from
/// the same store the effect uses (mirrors `pg_live_set`).
fn live_bank_set(store: &PostgresStore, bank: &str, subject: &str) -> Vec<String> {
    store.erase_bank_set(bank, subject).expect("live bank set")
}

/// One semantic item carrying one data subject.
fn subject_item(bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: unique("pg-coord-item"),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("persona").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"entity": "secret"})),
        gist: Some("secret gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Erase arguments bound to the real store, a journal, and a fresh salt.
fn erase_args<'a>(
    store: &'a PostgresStore,
    journal: &'a Path,
    ctx: &'a OpContext,
    sink: &'a EraseVecSink,
    request_id: &'a str,
    subject_id: &'a str,
    salt: &'a [u8],
) -> EraseArgs<'a> {
    EraseArgs {
        store,
        journal_path: journal,
        maint: None,
        ctx,
        subject_id,
        legal_basis: "GDPR_ART_17",
        request_id,
        reason_code: None,
        confirm: true,
        salt,
        timestamp: "2026-01-01T00:00:00Z",
        sink,
    }
}

/// A pending peer refuses on Postgres before any key loss or store effect,
/// and the subject's item stays readable.
#[test]
fn postgres_a_pending_peer_refuses_before_any_erasure_effect() {
    let Some(fixture) = pg_store_or_skip() else {
        return;
    };
    let record = two_bank(PeerState::Pending);
    let item = subject_item(BANK_A, SUBJECT);
    let peer_item = subject_item(BANK_B, SUBJECT);
    fixture
        .store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    fixture
        .store
        .create_memory_item(&peer_item, "tester")
        .expect("seed the peer bank item");
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let dir = FixtureDir::create(&unique("pg-coord-journal"));
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
    let live = live_bank_set(&fixture.store, BANK_A, SUBJECT);
    assert_eq!(live, vec![BANK_A.to_owned(), BANK_B.to_owned()]);
    let err =
        coordinated_erase_request(&args, &record, &live).expect_err("a pending peer must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
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
    assert!(
        fixture
            .store
            .get_memory_item(BANK_A, &item.id)
            .expect("readable")
            .is_some(),
        "the subject's item survives the refusal"
    );
}

/// A joined bank refuses on Postgres before any erasure effect.
#[test]
fn postgres_a_joined_bank_refuses_before_any_erasure_effect() {
    let Some(fixture) = pg_store_or_skip() else {
        return;
    };
    let record = two_bank(PeerState::Acknowledged);
    let item = subject_item(BANK_A, SUBJECT);
    let peer_item = subject_item(BANK_B, SUBJECT);
    let joined_bank = unique("pg-joined-bank");
    let joined_item = subject_item(&joined_bank, SUBJECT);
    fixture
        .store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    fixture
        .store
        .create_memory_item(&peer_item, "tester")
        .expect("seed the peer bank item");
    fixture
        .store
        .create_memory_item(&joined_item, "tester")
        .expect("seed the joined bank item");
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let dir = FixtureDir::create(&unique("pg-coord-journal"));
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
    let live = live_bank_set(&fixture.store, BANK_A, SUBJECT);
    assert_eq!(live.len(), 3, "the joined bank is live: {live:?}");
    let err =
        coordinated_erase_request(&args, &record, &live).expect_err("a joined bank must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        !fixture.store.subject_dek_destroyed(SUBJECT).expect("key"),
        "the refusal loses no key"
    );
    assert!(
        fixture
            .store
            .get_memory_item(BANK_A, &item.id)
            .expect("readable")
            .is_some(),
        "the subject's item survives the refusal"
    );
}

/// A departed bank refuses on Postgres before any erasure effect.
#[test]
fn postgres_a_departed_bank_refuses_before_any_erasure_effect() {
    let Some(fixture) = pg_store_or_skip() else {
        return;
    };
    let record = two_bank(PeerState::Acknowledged);
    let item = subject_item(BANK_A, SUBJECT);
    fixture
        .store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let dir = FixtureDir::create(&unique("pg-coord-journal"));
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
    let live = live_bank_set(&fixture.store, BANK_A, SUBJECT);
    assert_eq!(
        live,
        vec![BANK_A.to_owned()],
        "only the requesting bank is live"
    );
    let err =
        coordinated_erase_request(&args, &record, &live).expect_err("a departed bank must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        !fixture.store.subject_dek_destroyed(SUBJECT).expect("key"),
        "the refusal loses no key"
    );
}

/// Every peer acknowledged runs the real Postgres transaction and key store,
/// and a control subject in the same bank stays readable.
#[test]
fn postgres_every_peer_acknowledged_completes_and_keeps_the_control_subject() {
    let Some(fixture) = pg_store_or_skip() else {
        return;
    };
    let record = two_bank(PeerState::Acknowledged);
    let item = subject_item(BANK_A, SUBJECT);
    let peer_item = subject_item(BANK_B, SUBJECT);
    let control = subject_item(BANK_A, "control-subject");
    fixture
        .store
        .create_memory_item(&item, "tester")
        .expect("seed the subject item");
    fixture
        .store
        .create_memory_item(&peer_item, "tester")
        .expect("seed the peer bank item");
    fixture
        .store
        .create_memory_item(&control, "tester")
        .expect("seed the control subject item");
    let ctx = OpContext::new(Bank::new(BANK_A.to_owned()), Actor::Harness);
    let sink = EraseVecSink::default();
    let salt = rand::random::<[u8; 16]>();
    let dir = FixtureDir::create(&unique("pg-coord-journal"));
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
    let live = live_bank_set(&fixture.store, BANK_A, SUBJECT);
    assert_eq!(live, vec![BANK_A.to_owned(), BANK_B.to_owned()]);
    let outcome = coordinated_erase_request(&args, &record, &live)
        .expect("every coordinating bank confirmed");
    assert_eq!(outcome.status, "COMPLETED");
    assert!(
        fixture.store.subject_dek_destroyed(SUBJECT).expect("key"),
        "the erased subject's key is destroyed"
    );
    assert!(
        !fixture
            .store
            .read_tombstones(BANK_A, &record.request_id)
            .expect("tombstones")
            .is_empty(),
        "the committed purge leaves evidence"
    );
    assert!(
        fixture
            .store
            .get_memory_item(BANK_A, &control.id)
            .expect("readable")
            .is_some(),
        "the control subject remains readable"
    );
}

#[cfg(test)]
#[path = "subject_bank_coordination_multibank_pg_tests.rs"]
mod subject_bank_coordination_multibank_pg_tests;
