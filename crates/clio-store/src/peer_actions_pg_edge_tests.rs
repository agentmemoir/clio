// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the concurrent begin of the durable
//! per-peer action store on Postgres: two handles racing to begin one row key
//! must leave exactly one winner.
//!
//! ## Owns
//! - The two-handle Postgres begin-race assertions
//!
//! ## Does not own
//! - The scratch-Postgres fixture it runs on (`peer_actions_pg_tests`)
//! - The store contract and its driver SQL (`peer_actions`,
//!   `peer_actions_backends`)
//! - The SQLite race (`peer_actions_edge_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres begin stops serializing concurrent
//! writers on one row key.
//!
//! Test-only. Never compiled into production builds.

use super::{binding, open_pg, scratch_store, target, url_for_database};
use crate::peer_actions::{PeerActionDisposition, PeerActionState, PeerActionStore};
use crate::postgres::PostgresStore;
use clio_types::ErrorCode;

/// Two handles racing to begin one row key under different action ids leave
/// exactly one winner: the loser is refused and cannot advance the stored row
/// (the `durable_requests` race shape, over two Postgres clients).
#[test]
fn postgres_two_handles_racing_to_begin_one_row_leave_one_winner() {
    let _shared = crate::lock_pg_test();
    let Some((db, store)) = scratch_store("race") else {
        return;
    };
    let Some(second) = open_pg(&url_for_database(&db.name)) else {
        return;
    };
    let run = |store: PostgresStore, action_id: &'static str| {
        std::thread::spawn(move || {
            let outcome = store.begin(&binding("req-race", "peer-race", action_id));
            (store, outcome)
        })
    };
    let ((store, first_outcome), (_, second_outcome)) = (
        run(store, "act-a").join().expect("first begin thread"),
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
