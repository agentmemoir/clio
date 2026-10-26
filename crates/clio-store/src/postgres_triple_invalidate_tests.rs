// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres item invalidate close stamps
//! owned by `postgres_triple_read`: an item closed in the second it started
//! keeps a transaction interval that a read at that second still lands inside.
//!
//! ## Owns
//! - The same-second invalidate case through the `Store` entry point
//!
//! ## Does not own
//! - The item query and guard cases (see `postgres_triple_read_query_tests`)
//! - Atomic supersede writes and their close stamps (see `postgres_triple_tests`)
//!
//! ## Boundary
//!
//! Changes to the Postgres item invalidate close stamp should normally update
//! tests here.

use crate::PgTestGuard;
use crate::batch_test_support_tests::item;
use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::test_unique::unique;
use std::sync::Arc;

const START: &str = "2026-01-01T00:00:00Z";
const WIDENED: &str = "2026-01-01T00:00:01Z";

fn setup() -> Option<(PgTestGuard, PostgresStore)> {
    let guard = lock_pg_test();
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    match PostgresStore::open(&url, kms) {
        Ok(store) => Some((guard, store)),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres item invalidate tests", &err);
            None
        }
    }
}

#[test]
fn invalidate_item_in_its_starting_second_stores_one_second_later() {
    let Some((_guard, s)) = setup() else {
        return;
    };
    let bank = unique("b-pg-inv-same");
    let item_id = unique("item-pg-inv");
    let created = item(&item_id, &bank);
    assert_eq!(created.tx_from, START, "fixture starts the row at {START}");
    s.create_memory_item(&created, "agent").expect("create");
    let out = s
        .invalidate_item(&bank, &item_id, START, START, None, "agent")
        .expect("invalidate");
    assert_eq!(out.closed_ids, vec![item_id.clone()]);
    assert!(!out.already_ended);
    let row = s.get_memory_item(&bank, &item_id).unwrap().unwrap();
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
