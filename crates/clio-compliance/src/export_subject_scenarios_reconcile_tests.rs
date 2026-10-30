// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite open-destination and overwrite
//! scenarios of the subject export: reporting absent and landed open
//! destinations without writing or overwriting, sweeping the crash-leftover
//! managed staging file, and refusing an existing caller destination.
//!
//! ## Owns
//! - The SQLite reconcile, managed-leftover, and overwrite-refusal assertions
//!
//! ## Does not own
//! - The delivery driver and scratch fixtures (see
//!   `export_subject_scenarios_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::export_custody::export_custody_files::STAGING_DIR_NAMES;
use crate::export_custody::reconcile_open_destinations;
use crate::export_subject_scenarios_tests::{SubjectExport, accepted_controls, harness_ctx};
use crate::export_tests::{Scratch, seed_pending, staging_entries};
use crate::inventory::InventoryBackend;
use crate::inventory_map_fixtures::MapFixture;
use crate::test_unique::unique;
use clio_store::{DestinationCustodyStore, LocalEventStore};
use clio_types::ErrorCode;

#[test]
fn reconcile_reports_open_destinations_without_writing_or_overwriting() {
    let fixture = MapFixture::create();
    let scratch = Scratch::new();
    // A pending row whose destination is absent: a known open copy.
    let absent = scratch.destination();
    let absent_id = unique("crash-stage");
    seed_pending(&fixture.store, &fixture.store, &absent_id, &absent);
    // The crash left the managed staging file behind.
    let staging_root = std::env::temp_dir().join(STAGING_DIR_NAMES[0]);
    let staging_leftover = staging_root.join(format!("{absent_id}.staging"));
    std::fs::create_dir_all(&staging_root).expect("create the managed staging root");
    std::fs::write(&staging_leftover, b"leftover").expect("stage");
    // A pending row whose destination landed before the crash.
    let landed = scratch.root.join("landed.bin");
    std::fs::write(&landed, b"landed-bytes").expect("write the landed copy");
    let landed_id = unique("crash-landed");
    seed_pending(&fixture.store, &fixture.store, &landed_id, &landed);

    let open = reconcile_open_destinations(&fixture.store, &fixture.store).expect("reconcile");
    let reported: Vec<(&str, bool)> = open
        .iter()
        .map(|row| (row.action_id.as_str(), row.destination_present))
        .collect();
    assert_eq!(reported.len(), 2, "both open rows are reported");
    assert!(
        reported.contains(&(absent_id.as_str(), false)),
        "the absent destination is a known open copy: {reported:?}"
    );
    assert!(
        reported.contains(&(landed_id.as_str(), true)),
        "the landed destination is reported present: {reported:?}"
    );
    assert!(!absent.exists(), "reconciliation writes no destination");
    assert!(
        !staging_leftover.exists(),
        "reconciliation removes the crash-leftover managed staging file"
    );
    assert_eq!(
        std::fs::read(&landed).expect("read the landed copy"),
        b"landed-bytes",
        "reconciliation never overwrites or redelivers"
    );
    let pending = fixture.store.pending_actions().expect("pending");
    assert!(
        !pending.contains(&absent_id) && !pending.contains(&landed_id),
        "each open action's outcome was completed: {pending:?}"
    );
}

#[test]
fn subject_export_refuses_an_existing_destination_and_leaves_outside_files() {
    let fixture = MapFixture::create();
    let scratch = Scratch::new();
    let staging = scratch.staging_dir();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&staging).expect("create staging");
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&staging).expect("create staging");
    let foreign = staging.join("keep.bin");
    std::fs::write(&foreign, b"caller-staging").expect("write the foreign file");
    let dest = scratch.destination();
    std::fs::write(&dest, b"caller-bytes").expect("write the caller file");
    let controls = accepted_controls();
    let ctx = harness_ctx(&fixture.bank);
    let action_id = unique("exp-exists");
    let dest_str = dest.to_string_lossy().into_owned();
    let facts = fixture.facts();
    let err = SubjectExport {
        views: &fixture.store,
        ctx: &ctx,
        destination: &dest_str,
        staging_dir: &staging,
        facts: &facts,
        backend: InventoryBackend::Sqlite,
        controls: &controls,
        action_id: &action_id,
    }
    .run()
    .expect_err("an existing destination refuses");

    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        std::fs::read(&dest).expect("read the caller file"),
        b"caller-bytes",
        "the caller file is never overwritten"
    );
    assert_eq!(
        staging_entries(&staging),
        vec![foreign],
        "only the foreign staging file remains"
    );
    assert_eq!(
        fixture
            .store
            .load(&action_id)
            .expect("load")
            .expect("row")
            .state,
        "failed"
    );
}
