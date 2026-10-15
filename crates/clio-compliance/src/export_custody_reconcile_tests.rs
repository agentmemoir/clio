// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the reconciliation of open export destinations:
//! reporting each open copy without writing or overwriting it, completing each
//! pending outcome, and sweeping the managed staging file and destination temp
//! a crash left, by exact managed name only.
//!
//! ## Owns
//! - The reconcile assertions for open rows, pending outcomes, and managed
//!   leftovers
//!
//! ## Does not own
//! - The delivery refusals and success paths (see `export_custody_tests.rs`)
//! - The reconcile function under test
//!
//! ## Boundary
//!
//! Tests here break when the reconciliation report or the managed-leftover
//! sweep changes.
//!
//! Test-only. Never compiled into production builds.

use super::export_custody_files::{STAGING_DIR_NAMES, destination_temp_path, staging_root};
use super::export_custody_tests::{NOW, Scratch, open_sqlite};
use super::reconcile_open_destinations;
use crate::export_tests::seed_pending;
use crate::test_unique::unique;
use clio_store::{
    DestinationCustodyRow, DestinationCustodyStore, LocalEventIntent, LocalEventStore,
};

/// Reconciliation reports each open destination with its presence and never
/// rewrites an existing file.
#[test]
fn reconcile_reports_open_destinations_without_overwriting() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let present = scratch.destination();
    let absent = scratch.root.join("absent.bin");
    std::fs::write(&present, b"open-copy").expect("write the open copy");
    for action_id in ["act-absent", "act-present"] {
        store
            .record_intent(&LocalEventIntent {
                action_id,
                actor: "tester",
                scope: "export_custody",
                kind: "export_delivery",
                event_time: NOW,
            })
            .expect("record intent");
    }
    for (action_id, path) in [("act-present", &present), ("act-absent", &absent)] {
        store
            .save_pending(&DestinationCustodyRow {
                action_id: action_id.to_owned(),
                destination_path: path.display().to_string(),
                custody_owner: "clio".to_owned(),
                required_action: "delivery".to_owned(),
                state: "pending".to_owned(),
                exception_reason: None,
                updated_at: NOW.to_owned(),
            })
            .expect("save pending");
    }
    let open = reconcile_open_destinations(&store, &store).expect("reconcile");
    let reported: Vec<(&str, bool)> = open
        .iter()
        .map(|row| (row.action_id.as_str(), row.destination_present))
        .collect();
    assert_eq!(
        reported,
        vec![("act-absent", false), ("act-present", true)],
        "the open rows are reported in action-id order with their presence"
    );
    assert_eq!(
        std::fs::read(&present).expect("read the open copy"),
        b"open-copy",
        "reconciliation never rewrites an existing destination"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "each reconciled action's event outcome is completed"
    );
}

/// Reconciliation removes the managed staging file and the destination temp a
/// crash left for an open action, and never touches another file in the same
/// managed location.
#[test]
fn reconcile_sweeps_only_the_managed_leftovers_of_an_open_action() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let action_id = unique("act-sweep");
    let destination = scratch.destination();
    // A crash between staging and the link leaves the managed staging file.
    let staging_root = staging_root(STAGING_DIR_NAMES[0]);
    let staging_leftover = staging_root.join(format!("{action_id}.staging"));
    std::fs::create_dir_all(&staging_root).expect("create the managed staging root");
    std::fs::write(&staging_leftover, b"leftover").expect("seed the staging leftover");
    let foreign = staging_root.join(format!("{}.keep", unique("sweep-foreign")));
    std::fs::write(&foreign, b"caller-file").expect("seed the foreign file");
    // A crash between the link and the temp cleanup leaves the destination
    // temp beside the recorded destination.
    let temp_leftover = destination_temp_path(&destination, &action_id);
    std::fs::write(&temp_leftover, b"temp-leftover").expect("seed the temp leftover");
    seed_pending(&store, &store, &action_id, &destination);

    let open = reconcile_open_destinations(&store, &store).expect("reconcile");
    let reported: Vec<(&str, bool)> = open
        .iter()
        .map(|row| (row.action_id.as_str(), row.destination_present))
        .collect();
    assert_eq!(
        reported,
        vec![(action_id.as_str(), false)],
        "the open row is reported with an absent destination"
    );
    assert!(
        !staging_leftover.exists(),
        "the crash-leftover managed staging file is removed"
    );
    assert!(
        !temp_leftover.exists(),
        "the crash-leftover destination temp is removed"
    );
    assert!(
        foreign.exists(),
        "only managed names are swept: a foreign file in the staging root survives"
    );
    assert!(
        !destination.exists(),
        "reconciliation writes no destination"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the reconciled action's event outcome is completed"
    );
}
