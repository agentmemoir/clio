// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for what a discrete update stores on the
//! replacement item's authoritative snapshot half.
//!
//! ## Owns
//! - An object-valued update becoming the replacement's snapshot
//! - The prior snapshot not surviving onto an object-valued replacement
//!
//! ## Does not own
//! - The prose-value case and replacement linkage (see `update_tests`)
//! - Store persistence cases (see `clio-store` attribute suites)
//!
//! ## Boundary
//!
//! Changes to how a discrete update shapes the replacement snapshot should
//! normally update tests here. Changes to the continuous path should not
//! require changes here.
//!
//! Keep this module focused on the replacement's authoritative half.

use crate::triple::TripleVecSink;
use crate::update::update_tests::{args, item, store, unique};
use crate::update::{ContinuousVecSink, UpdateOutcome, update};
use clio_store::Store;
use clio_types::{Actor, Bank, OpContext, UpdateRule};

/// An object-valued update becomes the replacement's authoritative snapshot,
/// replacing the prior revision's.
#[test]
fn t_discrete_object_value_becomes_the_replacement_snapshot() {
    let s = store();
    let bank = unique("b");
    // The prior revision carries a snapshot: the case that must not survive.
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let cont = ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    let value = r#"{"entity":"new"}"#;
    let a = args(&s, &ctx, "i1", value, UpdateRule::Discrete, &cont, &triple);
    let replacement = match update(&a).unwrap() {
        UpdateOutcome::Discrete(out) => out
            .replacement_id
            .expect("discrete update must mint a replacement"),
        UpdateOutcome::Continuous(_) => panic!("discrete update must not EMA"),
    };
    let stored = s.get_memory_item(&bank, &replacement).unwrap().unwrap();
    assert_eq!(
        stored.snapshot,
        Some(serde_json::json!({"entity": "new"})),
        "an object value must become the authoritative snapshot"
    );
    assert_eq!(
        stored.gist.as_deref(),
        Some(value),
        "the value is also the prose half"
    );
}
