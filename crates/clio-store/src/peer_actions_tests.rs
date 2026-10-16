// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the durable per-peer action store on SQLite: the
//! idempotent begin, the guarded advance and its tolerated duplicates, the
//! fail-closed decoding of unknown stored spellings, and the verbatim
//! read-back across a reopen.
//!
//! ## Owns
//! - The SQLite begin/advance/restart assertions
//! - The unchecked-table fixture the fail-closed test installs
//!
//! ## Does not own
//! - The store contract (`peer_actions`) and its driver SQL
//!   (`peer_actions_backends`)
//! - The slot DDL text (`migrate` and `sql/003_local_compliance_slots.sql`)
//! - The Postgres backend assertions (`peer_actions_pg_tests`)
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
    PeerActionBinding, PeerActionDisposition, PeerActionKind, PeerActionRow, PeerActionState,
    PeerActionStore, PeerActionTarget,
};
use crate::sqlite::SqliteStore;
use clio_types::ErrorCode;

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

fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(local_kms()).expect("in-memory sqlite opens")
}

/// Begin and acknowledge one row, returning the acknowledged row.
fn acknowledged(store: &SqliteStore) -> PeerActionRow {
    store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin");
    store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RemovalEvidence,
                Some("generation=1"),
            ),
        )
        .expect("acknowledge")
}

/// A new begin inserts one pending row with no disposition or proof.
#[test]
fn a_begin_inserts_a_pending_row_and_a_duplicate_begin_is_a_no_op() {
    let store = open_sqlite();
    let first = store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin");
    assert_eq!(first.state, PeerActionState::Pending);
    assert_eq!(first.disposition, PeerActionDisposition::Unknown);
    assert_eq!(first.proof_ref, None);
    let again = store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("duplicate begin");
    assert_eq!(again, first, "the duplicate returns the stored row");
    // After an advance, a duplicate begin still cannot regress the row.
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
    let replayed = store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin after settle");
    assert_eq!(replayed, acked, "no state regression");
    assert_eq!(
        store.load_for_request("req-1").expect("list"),
        vec![acked],
        "one row only"
    );
}

/// A begin that rebinds the row key to a different idempotency id is refused
/// and changes nothing.
#[test]
fn a_begin_with_a_different_action_id_is_refused() {
    let store = open_sqlite();
    let first = store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin");
    let err = store
        .begin(&binding("req-1", "peer-1", "act-2"))
        .expect_err("a rebind must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store
            .load("req-1", "peer-1", PeerActionKind::Denial)
            .expect("load"),
        Some(first),
        "the stored row is untouched"
    );
}

/// An advance must match the stored binding and a legal transition; every
/// refused attempt leaves the row exactly as it was.
#[test]
fn an_advance_requires_the_stored_binding_and_a_legal_transition() {
    let store = open_sqlite();
    let pending = store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin");
    // A different action id is a binding conflict.
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
    // An unknown row key is not an insert.
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
        Some(pending.clone()),
        "nothing moved"
    );
    // The legal path carries its disposition and proof.
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
    assert_eq!(acked.disposition, PeerActionDisposition::RemovalEvidence);
    assert_eq!(acked.proof_ref.as_deref(), Some("generation=1"));
    // A terminal state cannot move again.
    let err = store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Refused,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect_err("acknowledged is terminal");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store
            .load("req-1", "peer-1", PeerActionKind::Denial)
            .expect("load"),
        Some(acked),
        "the refused move changed nothing"
    );
    // A refused row is terminal too.
    store
        .begin(&binding("req-2", "peer-2", "act-2"))
        .expect("begin");
    store
        .advance(
            &binding("req-2", "peer-2", "act-2"),
            &target(
                PeerActionState::Refused,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect("refuse");
    let err = store
        .advance(
            &binding("req-2", "peer-2", "act-2"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RemovalEvidence,
                Some("generation=2"),
            ),
        )
        .expect_err("refused is terminal");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// A duplicate acknowledgement is a tolerated no-op: it succeeds and the
/// stored row — proof included — is untouched, so a retry cannot repeat a
/// completed effect.
#[test]
fn a_duplicate_acknowledgement_is_a_tolerated_no_op() {
    let store = open_sqlite();
    let acked = acknowledged(&store);
    let retry = store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RetentionApproved,
                Some("other-proof"),
            ),
        )
        .expect("a duplicate acknowledgement succeeds");
    assert_eq!(retry, acked, "the stored row wins, proof included");
    // A duplicate delivery is a no-op the same way.
    store
        .begin(&binding("req-2", "peer-2", "act-2"))
        .expect("begin");
    let delivered = store
        .advance(
            &binding("req-2", "peer-2", "act-2"),
            &target(
                PeerActionState::Delivered,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect("deliver");
    let again = store
        .advance(
            &binding("req-2", "peer-2", "act-2"),
            &target(
                PeerActionState::Delivered,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect("duplicate delivery succeeds");
    assert_eq!(again, delivered, "the delivered row is settled once");
}

/// A stored row survives a reopen verbatim, and a duplicate acknowledgement
/// after the reopen is still a no-op.
#[test]
fn a_row_survives_a_reopen_and_stays_settled() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let acked = {
        let store = SqliteStore::open(&path, local_kms()).expect("open");
        acknowledged(&store)
    };
    let reopened = SqliteStore::open(&path, local_kms()).expect("reopen");
    assert_eq!(
        reopened
            .load("req-1", "peer-1", PeerActionKind::Denial)
            .expect("load"),
        Some(acked.clone()),
        "the restart read is verbatim"
    );
    let retry = reopened
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RemovalEvidence,
                Some("generation=1"),
            ),
        )
        .expect("duplicate acknowledgement after reopen");
    assert_eq!(retry, acked);
    assert_eq!(
        reopened.load_for_request("req-1").expect("list"),
        vec![acked],
        "still one row"
    );
}

/// An unrecognized stored enum spelling fails closed, for the state, the
/// disposition, and the action kind alike.
#[test]
fn unknown_stored_spellings_fail_closed() {
    let store = open_sqlite();
    store
        .with_conn(|conn| {
            conn.execute_batch(
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
                     ('req-kind', 'peer-c', 'void', 'act-3', 'pending', 'unknown', NULL);
                 INSERT INTO peer_actions VALUES
                     ('req-ok', 'peer-z', 'denial', 'act-4', 'pending', 'unknown', NULL);",
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
    assert_eq!(
        store
            .load("req-ok", "peer-z", PeerActionKind::Denial)
            .expect("a known spelling still loads")
            .expect("the row exists")
            .state,
        PeerActionState::Pending
    );
}

/// Scratch directory removed when the guard drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir("clio-store-peer-actions"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
