// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the copy
//! scenarios: every seeded copy carries its custody, owner, required action,
//! and state across a restart, and the metadata holds only the synthetic
//! values.
//!
//! ## Owns
//! - The scenario-1 restart assertions on the Postgres backend
//!
//! ## Does not own
//! - The wiring itself (`subject_request_store_wiring.rs`)
//! - The private-schema Postgres fixture and its skip note
//!   (`subject_request_store_wiring_pg_tests.rs`)
//! - The shared SQLite scenario fixtures
//!   (`subject_request_store_wiring_copy_scenarios_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres restart read stops returning the
//! stored copy metadata verbatim.
//!
//! Test-only. Never compiled into production builds.

use super::subject_request_store_wiring_copy_scenarios_tests::{copy_row, ctx};
use super::subject_request_store_wiring_pg_tests::pg_fixture_or_skip;
use super::subject_request_store_wiring_tests::{persist, started_record, subject_scope};
use super::{copy_row_closure_facts, record_copy_disposition, reload_ledger};
use crate::test_unique::unique;
use clio_store::CopyInventoryStore;

/// Every seeded copy carries its custody, owner, required action, and state
/// on the Postgres backend, and the metadata holds only the synthetic values.
#[test]
fn scenario_1_every_seeded_copy_carries_its_disposition_metadata_on_postgres() {
    let Some(mut fixture) = pg_fixture_or_skip() else {
        return;
    };
    let request_id = unique("pg-req");
    let rows = vec![
        copy_row("copy-staging", &request_id, "clio", "erase", "pending"),
        copy_row(
            "copy-caller-bundle",
            &request_id,
            "caller",
            "remove",
            "pending",
        ),
        copy_row(
            "copy-captured-log",
            &request_id,
            "operator",
            "remove",
            "pending",
        ),
        copy_row("copy-archive", &request_id, "operator", "erase", "pending"),
        copy_row(
            "copy-review-output",
            &request_id,
            "review_output",
            "erase",
            "pending",
        ),
    ];
    persist(
        &fixture.store,
        &started_record(&request_id, &subject_scope()),
    );
    let recorder = ctx();
    for row in &rows {
        record_copy_disposition(&recorder, &fixture.store, row).expect("the disposition records");
    }
    fixture.restart();
    let loaded = fixture.store.load_all().expect("the copy rows reload");
    let mut expected = rows;
    expected.sort_by(|a, b| a.copy_id.cmp(&b.copy_id));
    assert_eq!(
        loaded, expected,
        "metadata rows carry only the synthetic values"
    );
    let ledger = reload_ledger(&fixture.store).expect("the ledger reloads");
    let record = ledger.record(&request_id).expect("the record reloads");
    let facts = copy_row_closure_facts(record, &loaded, "product-path").expect("the facts derive");
    assert_eq!(
        facts.inputs.open_copy_actions,
        expected.len(),
        "every seeded open copy still blocks closure after the restart"
    );
}
