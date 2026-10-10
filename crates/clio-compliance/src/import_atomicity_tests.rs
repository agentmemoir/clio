// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the import atomicity regression suite: a
//! bundle that fails planning after many entity kinds were classified must
//! leave the store untouched (no write happens before the batch commit).
//!
//! ## Owns
//! - `planning_failure_writes_nothing` (regression)
//!
//! ## Does not own
//! - The import happy-path assertions (`import_tests.rs`)
//! - Fixture/entity builders (`import_fixture.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_store::{BeliefStore, HistoryStore, PersonaStore, Store};
use clio_types::ErrorCode;

use crate::bundle::ExportContentMode;
use crate::import::import;
use crate::import_fixture::{Harness, count_items, write_bundle};

#[test]
fn planning_failure_writes_nothing() {
    // Regression: a bundle that fails planning after items, opaque rows,
    // triples, assoc edges, persona rows, tasks, and failures were classified
    // must leave the store untouched (no writes happen until the batch commit).
    let h = Harness::new();
    let mut entities = h.full_entities();
    entities.opaque_items = h.opaque_entities().opaque_items;
    let mut bad_belief = entities.beliefs[0].clone();
    bad_belief.confidence_history.clear();
    entities.beliefs = vec![bad_belief];
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();

    let err = import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect_err("belief with no history fails planning");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    assert_eq!(count_items(&h), 0);
    assert!(
        h.store
            .get_preference(&h.bank, "brevity_affinity")
            .unwrap()
            .is_none()
    );
    let triples = h.store.list_triples(&h.bank).unwrap();
    assert!(triples.is_empty(), "{triples:?}");
    let edges = h.store.list_assoc_edges(&h.bank, None).unwrap();
    assert!(edges.is_empty(), "{edges:?}");
    let history = h
        .store
        .stable_history(&h.bank, "communication_style")
        .unwrap();
    assert!(history.is_empty(), "{history:?}");
    let tasks = h.store.list_tasks(&h.bank, 100).unwrap();
    assert!(tasks.is_empty(), "{tasks:?}");
    let failures = h.store.list_failures(&h.bank, 100).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let beliefs = h.store.list_beliefs(&h.bank).unwrap();
    assert!(beliefs.is_empty(), "{beliefs:?}");
}
