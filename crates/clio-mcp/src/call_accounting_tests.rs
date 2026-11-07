// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the call tally's own arithmetic: one record
//! call adds one to one name, and a snapshot is an ordered copy that a later
//! call cannot change.
//!
//! ## Owns
//! - `each_record_adds_one_to_that_name`
//! - `a_snapshot_is_ordered_by_name_and_taken_when_it_is_read`
//!
//! ## Does not own
//! - Which tool calls are recorded (see `protocol_call_accounting_tests`)
//! - Tool dispatch, argument checking, or result shaping (see `write_tools`)
//!
//! ## Boundary
//!
//! Changes to the tally's arithmetic should normally require changes here.
//! Changes to when a call is recorded should not: the execute path decides that.
//!
//! Keep this module focused on the tally's own arithmetic.

use super::DirectCallAccounting;
use std::collections::BTreeMap;

#[test]
fn each_record_adds_one_to_that_name() {
    let calls = DirectCallAccounting::default();
    calls.record("stats");
    calls.record("stats");
    calls.record("get");

    assert_eq!(
        calls.snapshot(),
        BTreeMap::from([("get".to_owned(), 1), ("stats".to_owned(), 2)])
    );
}

#[test]
fn a_snapshot_is_ordered_by_name_and_taken_when_it_is_read() {
    let calls = DirectCallAccounting::default();
    calls.record("retrieve");
    let before = calls.snapshot();
    calls.record("get");

    let names: Vec<&str> = before.keys().map(String::as_str).collect();
    assert_eq!(names, ["retrieve"], "a snapshot is a copy: {before:?}");
    assert_eq!(calls.snapshot().len(), 2);
}
