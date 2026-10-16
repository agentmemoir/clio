// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the durable per-peer action store on Postgres: the
//! idempotent begin and read-back after a reopen, the guarded advance and its
//! tolerated duplicates, the fail-closed decoding of unknown spellings, and the
//! blank-binding and out-of-domain refusals.
//!
//! ## Owns
//! - The scratch-Postgres database fixture and its drop guard
//! - The Postgres begin/advance/reopen/refusal assertions, including the
//!   blank-binding and out-of-domain refusals and the multi-row read order
//!
//! ## Does not own
//! - The store contract (`peer_actions`) and its driver SQL
//!   (`peer_actions_backends`)
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - Shared-Postgres test serialization (`pg_lock`)
//! - The SQLite backend assertions (`peer_actions_tests`)
//! - The two-handle begin race (`peer_actions_pg_edge_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the begin/advance rules, the read-back shape, or the
//! restart behavior changes. The request record and the sync transport belong
//! to their own suites.
//!
//! Test-only. Never compiled into production builds.

use crate::erase_test_support::local_kms;
use crate::migrate::IntoAmError;
use crate::peer_actions::{
    PeerActionBinding, PeerActionDisposition, PeerActionKind, PeerActionState, PeerActionStore,
    PeerActionTarget,
};
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode};

/// The suite name every fail-closed skip in this module carries.
const SUITE: &str = "postgres peer action tests";

/// The fixed row key every test begins.
fn binding<'a>(request: &'a str, peer: &'a str, action_id: &'a str) -> PeerActionBinding<'a> {
    PeerActionBinding {
        request_id: request,
        peer_id: peer,
        action_kind: PeerActionKind::Denial,
        action_id,
    }
}

/// A target with the given state, disposition, and proof.
fn target(
    to: PeerActionState,
    disposition: PeerActionDisposition,
    proof_ref: Option<&str>,
) -> PeerActionTarget<'_> {
    PeerActionTarget {
        to,
        disposition,
        proof_ref,
    }
}

/// The shared Postgres URL, or `None` after the fail-closed skip line. Only
/// the explicit `CLIO_SKIP_PG_TESTS` opt-out reaches that line: without it an
/// unreachable server fails the suite, so this backend cannot report green
/// without touching a database. The note names the suite and the server's
/// reason, never the connection string (the `durable_requests_pg_tests` shape).
fn shared_url() -> Option<String> {
    let url = crate::db_url::postgres_url_from_env();
    if let Err(err) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip(SUITE, &err);
        return None;
    }
    Some(url)
}

/// A database created for one test, dropped when the test ends — including
/// when it ends in a panic (the `durable_requests_pg_tests` fixture shape).
struct ScratchDatabase {
    name: String,
}

impl ScratchDatabase {
    fn create(admin: &mut postgres::Client, label: &str) -> Self {
        // The crate's unique helper, made identifier-safe: a Postgres database
        // name cannot carry the helper's dashes unquoted.
        let name = crate::test_unique::unique(&format!("clio_peer_act_{label}")).replace('-', "_");
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name}"))
            .expect("drop a leftover scratch database");
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .expect("create a scratch database");
        Self { name }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        if let Ok(mut admin) = crate::pg_tls::connect(&crate::db_url::postgres_url_from_env()) {
            let _ = admin.batch_execute(&format!("DROP DATABASE IF EXISTS {}", self.name));
        }
    }
}

/// The shared URL pointed at a different database on the same server.
fn url_for_database(name: &str) -> String {
    crate::db_url::with_database(&crate::db_url::postgres_url_from_env(), name)
}

fn open_pg(url: &str) -> Option<PostgresStore> {
    match PostgresStore::open(url, local_kms()) {
        Ok(store) => Some(store),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip(SUITE, &err);
            None
        }
    }
}

/// A started store over a fresh scratch database.
///
/// The caller takes `lock_pg_test()` itself and holds it for the whole test:
/// the guard cannot live here, because it would release the shared-database
/// serialization the moment this helper returned (see `pg_lock`).
fn scratch_store(label: &str) -> Option<(ScratchDatabase, PostgresStore)> {
    let url = shared_url()?;
    let mut admin = crate::pg_tls::connect(&url).expect("admin connect");
    let db = ScratchDatabase::create(&mut admin, label);
    drop(admin);
    let store = open_pg(&url_for_database(&db.name))?;
    Some((db, store))
}

/// A row saves, acknowledges, and reads back identically after a reopen; a
/// duplicate acknowledgement after the reopen is still a no-op.
#[test]
fn postgres_a_row_saves_and_reads_back_after_a_reopen() {
    let _shared = crate::lock_pg_test();
    let Some((db, store)) = scratch_store("roundtrip") else {
        return;
    };
    store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin");
    let acked = store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RemovalEvidence,
                Some("generation=1"),
            ),
        )
        .expect("acknowledge");
    drop(store);

    let reopened = open_pg(&url_for_database(&db.name)).expect("reopen");
    assert_eq!(
        reopened
            .load("req-1", "peer-1", PeerActionKind::Denial)
            .expect("load"),
        Some(acked.clone()),
        "the row restarts verbatim"
    );
    // A retried begin after the reopen is a no-op, not a regression.
    assert_eq!(
        reopened
            .begin(&binding("req-1", "peer-1", "act-1"))
            .expect("duplicate begin after reopen"),
        acked
    );
    let retry = reopened
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RetentionApproved,
                Some("other-proof"),
            ),
        )
        .expect("duplicate acknowledgement succeeds");
    assert_eq!(retry, acked, "the completed row cannot repeat");
    assert_eq!(
        reopened.load_for_request("req-1").expect("list"),
        vec![acked],
        "one row only"
    );
}

/// An advance must match the stored binding and a legal transition; every
/// refused attempt leaves the row exactly as it was.
#[test]
fn postgres_a_guarded_advance_refuses_a_mismatched_binding() {
    let _shared = crate::lock_pg_test();
    let Some((_db, store)) = scratch_store("guard") else {
        return;
    };
    let pending = store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin");
    let err = store
        .advance(
            &binding("req-1", "peer-1", "act-9"),
            &target(
                PeerActionState::Delivered,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect_err("a different action id must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = store
        .advance(
            &binding("req-1", "peer-9", "act-1"),
            &target(
                PeerActionState::Delivered,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect_err("an unknown peer must refuse");
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(
        store
            .load("req-1", "peer-1", PeerActionKind::Denial)
            .expect("load"),
        Some(pending),
        "nothing moved"
    );
    let delivered = store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Delivered,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect("deliver");
    assert_eq!(delivered.state, PeerActionState::Delivered);
    let err = store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RemovalEvidence,
                Some(""),
            ),
        )
        .expect_err("a blank proof reference must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let acked = store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RemovalEvidence,
                Some("generation=1"),
            ),
        )
        .expect("acknowledge");
    assert_eq!(acked.state, PeerActionState::Acknowledged);
    assert_eq!(acked.disposition, PeerActionDisposition::RemovalEvidence);
    assert_eq!(acked.proof_ref.as_deref(), Some("generation=1"));
}

/// An unrecognized stored enum spelling fails closed on Postgres too, for the
/// state, the disposition, and the action kind alike.
#[test]
fn postgres_unknown_stored_spellings_fail_closed() {
    let _shared = crate::lock_pg_test();
    let Some((_db, store)) = scratch_store("spelling") else {
        return;
    };
    store
        .with_client(|client| {
            client
                .batch_execute(
                    "DROP TABLE peer_actions;
                     CREATE TABLE peer_actions
                     (
                         request_id text NOT NULL,
                         peer_id text NOT NULL,
                         action_kind text NOT NULL,
                         action_id text NOT NULL,
                         state text NOT NULL,
                         disposition text NOT NULL,
                         proof_ref text,
                         PRIMARY KEY (request_id, peer_id, action_kind)
                     );
                     INSERT INTO peer_actions VALUES
                         ('req-state', 'peer-a', 'denial', 'act-1', 'lost', 'unknown', NULL);
                     INSERT INTO peer_actions VALUES
                         ('req-disposition', 'peer-b', 'denial', 'act-2', 'pending', 'kept', NULL);
                     INSERT INTO peer_actions VALUES
                         ('req-kind', 'peer-c', 'void', 'act-3', 'pending', 'unknown', NULL);",
                )
                .am("install the unchecked peer_actions table")?;
            Ok(())
        })
        .expect("install the fixture rows");
    let err = store
        .load("req-state", "peer-a", PeerActionKind::Denial)
        .expect_err("an unknown state must fail closed");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    let err = store
        .load("req-disposition", "peer-b", PeerActionKind::Denial)
        .expect_err("an unknown disposition must fail closed");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    let err = store
        .load_for_request("req-kind")
        .expect_err("an unknown action kind must fail closed");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
}

/// A binding with a blank required field is refused before any statement runs,
/// and the refusal names the field.
#[test]
fn postgres_a_blank_binding_is_refused() {
    let _shared = crate::lock_pg_test();
    let Some((_db, store)) = scratch_store("blank") else {
        return;
    };
    for (field, blank) in [
        ("request_id", binding(" ", "peer-1", "act-1")),
        ("peer_id", binding("req-1", "", "act-1")),
        ("action_id", binding("req-1", "peer-1", "\t")),
    ] {
        let err = store.begin(&blank).expect_err("a blank field must refuse");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{field}");
        assert!(
            err.message.contains(field),
            "the refusal names {field}: {}",
            err.message
        );
    }
    assert!(
        store.load_for_request("req-1").expect("list").is_empty(),
        "no refused binding left a row"
    );
}

/// The one-request read orders the rows by peer id, then action kind: every
/// other read in this suite holds a single row.
#[test]
fn postgres_load_for_request_orders_two_rows_by_peer_id() {
    let _shared = crate::lock_pg_test();
    let Some((_db, store)) = scratch_store("order") else {
        return;
    };
    store
        .begin(&binding("req-order", "peer-b", "act-b"))
        .expect("begin peer-b");
    store
        .begin(&binding("req-order", "peer-a", "act-a"))
        .expect("begin peer-a");
    let rows = store
        .load_for_request("req-order")
        .expect("list the ordered rows");
    let peers: Vec<&str> = rows.iter().map(|row| row.peer_id.as_str()).collect();
    assert_eq!(peers, ["peer-a", "peer-b"], "ordered by peer id");
    assert_eq!(rows.len(), 2, "both rows are returned");
}

/// The DDL's CHECK domains refuse an out-of-domain state through the table
/// itself, without the recreated unchecked fixture the fail-closed test needs.
#[test]
fn postgres_the_check_domains_refuse_an_out_of_domain_state() {
    let _shared = crate::lock_pg_test();
    let Some((_db, store)) = scratch_store("domain") else {
        return;
    };
    let refusal = store
        .with_client(|client| {
            client
                .execute(
                    "INSERT INTO peer_actions
                         (request_id, peer_id, action_kind, action_id, state, disposition, proof_ref)
                     VALUES ('req-domain', 'peer-a', 'denial', 'act-1', 'lost', 'unknown', NULL)",
                    &[],
                )
                .map(|_| ())
                .map_err(|err| AmError::new(ErrorCode::InvalidArgument, format!("{err:?}")))
        })
        .expect_err("an out-of-domain state must be refused");
    assert!(
        refusal.message.contains("check constraint") && refusal.message.contains("23514"),
        "the refusal is the CHECK domain's (SQLSTATE 23514): {}",
        refusal.message
    );
    assert!(
        store
            .load_for_request("req-domain")
            .expect("list")
            .is_empty(),
        "the refused row is not stored"
    );
}

#[path = "peer_actions_pg_edge_tests.rs"]
mod edge;
