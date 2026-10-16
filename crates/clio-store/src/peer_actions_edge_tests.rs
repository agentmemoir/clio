// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the edge cases of the durable per-peer
//! action store on SQLite: the two-handle begin race, the refused blank
//! bindings and targets, the out-of-domain write the DDL's CHECK refuses, and
//! the multi-row read order.
//!
//! ## Owns
//! - The SQLite two-handle begin-race assertions
//! - The SQLite blank-binding, blank-proof, and out-of-domain refusals, and
//!   the two-row read order
//!
//! ## Does not own
//! - The fixture and the single-row begin/advance assertions
//!   (`peer_actions_tests`)
//! - The store contract and its driver SQL (`peer_actions`,
//!   `peer_actions_backends`)
//! - The Postgres edges (`peer_actions_pg_edge_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the begin stops serializing concurrent writers, when
//! validation stops refusing blank fields, or when the DDL guard or the read
//! order changes.
//!
//! Test-only. Never compiled into production builds.

use super::{Scratch, binding, open_sqlite, target};
use crate::erase_test_support::local_kms;
use crate::migrate::IntoAmError;
use crate::peer_actions::{
    PeerActionDisposition, PeerActionKind, PeerActionState, PeerActionStore,
};
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};

/// Two file-backed handles racing to begin one row key under different action
/// ids leave exactly one winner: the loser is refused, the stored row is the
/// winner's, and an advance carrying the loser's action id is refused (the
/// `durable_requests` race shape, over two stores on one file).
#[test]
fn two_handles_racing_to_begin_one_row_leave_one_winner() {
    use std::time::Duration;

    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let first = SqliteStore::open(&path, local_kms()).expect("first handle");
    let second = SqliteStore::open(&path, local_kms()).expect("second handle");
    // Both handles write: a bounded busy wait lets the loser reach the
    // winner's committed row instead of `SQLITE_BUSY`.
    for store in [&first, &second] {
        store
            .with_conn(|conn| {
                conn.busy_timeout(Duration::from_secs(10))
                    .am("peer action race busy timeout")?;
                Ok(())
            })
            .expect("set the write-contention wait");
    }
    let run = |store: SqliteStore, action_id: &'static str| {
        std::thread::spawn(move || {
            let outcome = store.begin(&binding("req-race", "peer-race", action_id));
            (store, outcome)
        })
    };
    let ((store, first_outcome), (_, second_outcome)) = (
        run(first, "act-a").join().expect("first begin thread"),
        run(second, "act-b").join().expect("second begin thread"),
    );
    let (winner, loser_id) = match (first_outcome, second_outcome) {
        (Ok(row), Err(loser)) => {
            assert_eq!(
                loser.code,
                ErrorCode::InvalidArgument,
                "the loser is refused"
            );
            (row, "act-b")
        }
        (Err(loser), Ok(row)) => {
            assert_eq!(
                loser.code,
                ErrorCode::InvalidArgument,
                "the loser is refused"
            );
            (row, "act-a")
        }
        _ => panic!("exactly one handle must win the begin"),
    };
    assert_eq!(
        store
            .load_for_request("req-race")
            .expect("list the raced rows"),
        vec![winner],
        "one row only, the winner's"
    );
    let err = store
        .advance(
            &binding("req-race", "peer-race", loser_id),
            &target(
                PeerActionState::Delivered,
                PeerActionDisposition::Unknown,
                None,
            ),
        )
        .expect_err("the loser cannot advance the winner's row");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// A binding with a blank required field is refused before any statement runs,
/// and the refusal names the field.
#[test]
fn a_blank_binding_is_refused() {
    let store = open_sqlite();
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

/// A target whose proof reference is present but blank is refused on SQLite
/// too, so the same guard covers both drivers, and the row is untouched.
#[test]
fn a_blank_proof_reference_is_refused() {
    let store = open_sqlite();
    store
        .begin(&binding("req-1", "peer-1", "act-1"))
        .expect("begin");
    let err = store
        .advance(
            &binding("req-1", "peer-1", "act-1"),
            &target(
                PeerActionState::Acknowledged,
                PeerActionDisposition::RemovalEvidence,
                Some(" "),
            ),
        )
        .expect_err("a blank proof reference must refuse");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        store
            .load("req-1", "peer-1", PeerActionKind::Denial)
            .expect("load")
            .expect("the row exists")
            .state,
        PeerActionState::Pending,
        "the refusal changed nothing"
    );
}

/// The one-request read orders the rows by peer id, then action kind: every
/// other read in this suite holds a single row.
#[test]
fn load_for_request_orders_two_rows_by_peer_id() {
    let store = open_sqlite();
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
fn the_check_domains_refuse_an_out_of_domain_state() {
    let store = open_sqlite();
    let refusal = store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO peer_actions
                     (request_id, peer_id, action_kind, action_id, state, disposition, proof_ref)
                 VALUES ('req-domain', 'peer-a', 'denial', 'act-1', 'lost', 'unknown', NULL)",
                [],
            )
            .map(|_| ())
            .map_err(|err| AmError::new(ErrorCode::InvalidArgument, format!("{err:?}")))
        })
        .expect_err("an out-of-domain state must be refused");
    assert!(
        refusal.message.contains("CHECK constraint failed") && refusal.message.contains("275"),
        "the refusal is the CHECK domain's (SQLITE_CONSTRAINT_CHECK 275): {}",
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
