// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twins of the restart
//! coverage: restart replay of the stored state, saved-path survival with
//! its attempt and action ids, and scope-reuse refusal without mutation.
//!
//! ## Owns
//! - The private-schema Postgres fixture these tests open and reopen
//! - The restart assertions on the Postgres backend
//!
//! ## Does not own
//! - The wiring itself (`subject_request_store_wiring.rs`)
//! - The shared SQLite fixtures (`subject_request_store_wiring_tests.rs`)
//! - The durable erasure seam (`subject_request_store_wiring_erase_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres restart read stops returning the
//! stored record verbatim.
//!
//! Test-only. Never compiled into production builds.

use super::subject_request_store_wiring_tests::{
    kms, persist, started_record, subject_scope, wiring_item,
};
use crate::inventory_pg_support_tests::{pg_checked_url, pg_skip_note};
use crate::subject_request::{
    PathOwner, PathSlot, PathState, RegisterOutcome, RequestKind, RequestScope, RequestState,
};
use crate::subject_request_store_wiring::{record_to_row, reload_ledger};
use crate::test_unique::unique;
use clio_store::{
    DurableRequestStore, PgTestGuard, PostgresStore, ScopedSchema, Store, lock_pg_test,
    probe_postgres,
};
use clio_types::{AmError, ErrorCode};

/// A private-schema Postgres store; the store drops before the schema
/// (`subject_bundle_pg_tests.rs` fixture shape). The copy-scenario twin
/// reopens the same fixture.
pub(super) struct PgFixture {
    pub(super) store: PostgresStore,
    schema: ScopedSchema,
    _guard: PgTestGuard,
}

impl PgFixture {
    /// Close the store and reopen the same schema: the restart boundary.
    pub(super) fn restart(&mut self) {
        self.store = PostgresStore::open(self.schema.url(), kms())
            .expect("reopen the fixture store on the same scoped schema");
    }

    /// Run one batch of SQL against the fixture's private schema. The
    /// fault-injection a test needs (a trigger) has no store API to reach it,
    /// so the fixture's own schema connection carries it.
    pub(super) fn execute(&self, sql: &str) -> Result<(), AmError> {
        self.schema.execute(sql)
    }
}

/// The suite name every skip note in this module carries.
const SUITE: &str = "subject request wiring Postgres tests";

/// True only when the operator explicitly opted out of Postgres-backed tests.
/// Without this opt-out an unreachable Postgres fails the caller instead of
/// silently reporting green.
fn skip_requested() -> bool {
    matches!(
        std::env::var("CLIO_SKIP_PG_TESTS").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

/// Open the live Postgres fixture. An unreachable Postgres panics unless the
/// operator set `CLIO_SKIP_PG_TESTS`, which prints the skip note and returns
/// `None`. Neither the skip note nor the panic carries a connection string.
pub(super) fn pg_fixture_or_skip() -> Option<PgFixture> {
    let guard = lock_pg_test();
    let base = pg_checked_url();
    if let Err(detail) = probe_postgres(&base) {
        let err = AmError::new(ErrorCode::Internal, format!("postgres probe: {detail}"));
        if skip_requested() {
            eprintln!("{}", pg_skip_note(SUITE, &err));
            return None;
        }
        panic!("{SUITE} require a reachable Postgres: {err}");
    }
    let schema = ScopedSchema::new(&base);
    let store = PostgresStore::open(schema.url(), kms()).expect("open the scoped fixture store");
    Some(PgFixture {
        store,
        schema,
        _guard: guard,
    })
}

/// A request started on Postgres replays its stored state when the same id
/// and scope are submitted after a restart.
#[test]
fn postgres_a_started_request_replays_its_stored_state_after_a_restart() {
    let Some(mut fixture) = pg_fixture_or_skip() else {
        return;
    };
    let request_id = unique("pg-req");
    let scope = subject_scope();
    persist(&fixture.store, &started_record(&request_id, &scope));
    fixture.restart();
    let mut ledger = reload_ledger(&fixture.store).expect("the ledger reloads");
    assert!(
        matches!(
            ledger.register(&request_id, RequestKind::Erasure, &scope, &[]),
            Ok(RegisterOutcome::Replayed {
                state: RequestState::InProgress
            })
        ),
        "the same id and scope must replay the stored state"
    );
}

/// A saved path slot restarts on Postgres with its attempt and action ids,
/// with nothing auto-completed.
#[test]
fn postgres_a_saved_path_restarts_with_its_attempt_and_action_ids() {
    let Some(mut fixture) = pg_fixture_or_skip() else {
        return;
    };
    let request_id = unique("pg-req");
    let scope = subject_scope();
    {
        let mut record = started_record(&request_id, &scope);
        record.paths.insert(
            "p1".into(),
            PathSlot {
                owner: PathOwner::Clio,
                state: PathState::Saved {
                    attempt_id: "att-1".into(),
                    action_id: "act-1".into(),
                },
            },
        );
        persist(&fixture.store, &record);
    }
    fixture.restart();
    let ledger = reload_ledger(&fixture.store).expect("the ledger reloads");
    let restarted = ledger.record(&request_id).expect("the record reloads");
    assert_eq!(
        restarted.paths.get("p1").map(|slot| &slot.state),
        Some(&PathState::Saved {
            attempt_id: "att-1".into(),
            action_id: "act-1".into(),
        }),
        "the saved path must restart verbatim"
    );
}

/// A reused id bound to a changed scope refuses on Postgres and the stored
/// row is untouched.
#[test]
fn postgres_a_reused_id_with_a_changed_scope_refuses_without_mutation() {
    let Some(mut fixture) = pg_fixture_or_skip() else {
        return;
    };
    let request_id = unique("pg-req");
    let scope = subject_scope();
    let record = started_record(&request_id, &scope);
    let stored_row = record_to_row(&record);
    persist(&fixture.store, &record);
    fixture.restart();
    let mut ledger = reload_ledger(&fixture.store).expect("the ledger reloads");
    let changed = RequestScope::Bank {
        bank_id: unique("wiring-bank"),
    };
    let err = ledger
        .register(&request_id, RequestKind::Erasure, &changed, &[])
        .expect_err("a changed scope must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(
        fixture
            .store
            .load(&request_id)
            .expect("load")
            .expect("row exists"),
        stored_row,
        "the stored row must be untouched"
    );
}

/// The Postgres store reports the same fan-out set the erasure reads: the
/// requesting bank plus every bank holding the subject's items.
#[test]
fn postgres_the_store_reports_the_erasure_fan_out_bank_set() {
    let Some(fixture) = pg_fixture_or_skip() else {
        return;
    };
    let bank = unique("pg-wiring-bank");
    let other_bank = unique("pg-wiring-bank");
    let subject = unique("subject");
    fixture
        .store
        .create_memory_item(&wiring_item(&bank, &subject), "tester")
        .expect("write the subject item");
    fixture
        .store
        .create_memory_item(&wiring_item(&other_bank, &subject), "tester")
        .expect("write the peer-bank item");
    let mut expected = vec![bank, other_bank];
    expected.sort();
    assert_eq!(
        fixture
            .store
            .erase_bank_set(&expected[0], &subject)
            .expect("read the fan-out set"),
        expected,
        "the live set must cover every bank sharing the key"
    );
}

// The live-Postgres twins of the retry suites, declared here so they reach
// this module's `pub(super)` fixture while `subject_request_store_wiring`
// stays within its line cap.
#[cfg(test)]
#[path = "subject_request_store_wiring_erase_pg_tests.rs"]
mod subject_request_store_wiring_erase_pg_tests;

#[cfg(test)]
#[path = "erasure_retry_pg_tests.rs"]
mod erasure_retry_pg_tests;

#[cfg(test)]
#[path = "erasure_retry_race_pg_tests.rs"]
mod erasure_retry_race_pg_tests;

// The remaining live-Postgres fault-point twins, declared here for the same
// reason as the retry suites above.
#[cfg(test)]
#[path = "erasure_retry_fault_pg_tests.rs"]
mod erasure_retry_fault_pg_tests;
