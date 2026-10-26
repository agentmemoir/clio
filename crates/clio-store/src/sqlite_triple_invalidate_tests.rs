// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite item invalidate close stamps owned
//! by `sqlite_triple_invalidate`: an item closed in the second it started keeps
//! a transaction interval that a read at that second still lands inside.
//!
//! ## Owns
//! - The same-second invalidate case through the `Store` entry point
//!
//! ## Does not own
//! - The batch executor's own all-or-nothing cases (see `batch_model_tests`)
//! - The item query and guard cases (see `sqlite_triple_read_query_tests`)
//!
//! ## Boundary
//!
//! Changes to the item invalidate close stamp should normally update tests here.

use crate::batch_test_support_tests::{item, store};
use crate::store::Store;
use crate::test_unique::unique;

const START: &str = "2026-01-01T00:00:00Z";
const WIDENED: &str = "2026-01-01T00:00:01Z";

#[test]
fn invalidate_item_in_its_starting_second_stores_one_second_later() {
    let s = store();
    let bank = unique("b-inv-same");
    let created = item("item-a", &bank);
    assert_eq!(created.tx_from, START, "fixture starts the row at {START}");
    s.create_memory_item(&created, "agent").expect("create");
    let out = s
        .invalidate_item(&bank, "item-a", START, START, None, "agent")
        .expect("invalidate");
    assert_eq!(out.closed_ids, vec!["item-a".to_owned()]);
    assert!(!out.already_ended);
    let row = s.get_memory_item(&bank, "item-a").unwrap().unwrap();
    assert_eq!(
        row.tx_until.as_deref(),
        Some(WIDENED),
        "close lands one second on"
    );
    assert_eq!(
        row.valid_until.as_deref(),
        Some(START),
        "valid axis unchanged"
    );
    let until = row.tx_until.clone().expect("row is closed");
    assert!(
        row.tx_from < until,
        "a transaction read at {} sits inside [{until})",
        row.tx_from
    );
}

#[test]
fn invalidate_item_after_the_row_start_keeps_the_requested_stamp() {
    let s = store();
    let bank = unique("b-inv-later");
    let created = item("item-a", &bank);
    s.create_memory_item(&created, "agent").expect("create");
    s.invalidate_item(
        &bank,
        "item-a",
        START,
        "2026-03-01T00:00:00Z",
        None,
        "agent",
    )
    .expect("invalidate");
    let row = s.get_memory_item(&bank, "item-a").unwrap().unwrap();
    assert_eq!(row.tx_until.as_deref(), Some("2026-03-01T00:00:00Z"));
}
