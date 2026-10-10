// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres/SQLite parity test of the
//! bi-temporal item window query.
//!
//! ## Owns
//! - The shared parity fixture and the ordered-id comparison across backends
//!
//! ## Does not own
//! - SQLite-only window behavior (see `sqlite_window_tests`)
//! - Production SQL (see `window_query` and the backend window modules)
//!
//! ## Boundary
//!
//! Changes to cross-backend window parity expectations should update tests here.
//! Changes to SQLite-only behavior should not require changes here.
//!
//! Keep this module focused on Postgres window parity.

use crate::db_url::postgres_url_from_env;
use crate::postgres::PostgresStore;
use crate::sqlite_window::sqlite_window_tests::{END, START, fixture, open_sqlite};
use crate::test_unique::unique;
use crate::{DekProvider, HygieneStore, LocalDevKms, Store};
use clio_types::{MemoryItem, TimeAxis};
use std::sync::Arc;

/// The item set seeded into both backends for the parity comparison.
fn parity_fixture(bank: &str) -> Vec<MemoryItem> {
    vec![
        // Valid axis: open-ended item started before the window.
        fixture(
            &format!("{bank}-open"),
            bank,
            "2024-01-15T00:00:00Z",
            None,
            "2024-01-01T00:00:00Z",
            None,
        ),
        // Valid axis: interval closes inside the window.
        fixture(
            &format!("{bank}-closed"),
            bank,
            "2024-03-05T00:00:00Z",
            Some("2024-03-25T00:00:00Z"),
            "2024-02-01T00:00:00Z",
            None,
        ),
        // Valid axis: interval starts inside the window and stays open.
        fixture(
            &format!("{bank}-live"),
            bank,
            "2024-03-15T00:00:00Z",
            None,
            "2024-02-02T00:00:00Z",
            None,
        ),
        // Superseded belief: never eligible.
        fixture(
            &format!("{bank}-gone"),
            bank,
            "2024-03-12T00:00:00Z",
            None,
            "2024-02-03T00:00:00Z",
            Some("2024-02-04T00:00:00Z"),
        ),
        // Valid axis: starts exactly at the exclusive window end.
        fixture(
            &format!("{bank}-late"),
            bank,
            END,
            None,
            "2024-02-05T00:00:00Z",
            None,
        ),
        // Transaction axis: recorded before the window and still open.
        fixture(
            &format!("{bank}-tx-open"),
            bank,
            "2020-01-01T00:00:00Z",
            Some("2020-02-01T00:00:00Z"),
            "2024-02-10T00:00:00Z",
            None,
        ),
        // Transaction axis: recorded inside the window.
        fixture(
            &format!("{bank}-tx-inside"),
            bank,
            "2020-01-01T00:00:00Z",
            Some("2020-02-01T00:00:00Z"),
            "2024-03-20T00:00:00Z",
            None,
        ),
        // Archived fixture: in-window on the valid axis (open-ended) and a
        // transaction-axis candidate by tx columns. Archived below, its id is
        // absent from `valid_expected` and `tx_expected`; the row proves the
        // new `archived_at IS NULL` clause agrees across backends.
        fixture(
            &format!("{bank}-archived"),
            bank,
            "2024-03-18T00:00:00Z",
            None,
            "2024-02-06T00:00:00Z",
            None,
        ),
        // Transaction axis: recorded exactly at the exclusive window end.
        fixture(
            &format!("{bank}-tx-late"),
            bank,
            "2020-01-01T00:00:00Z",
            Some("2020-02-01T00:00:00Z"),
            END,
            None,
        ),
    ]
}

#[test]
fn window_ids_match_between_postgres_and_sqlite() {
    let _guard = crate::lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    // Never echo the URL: it can carry credentials and test output can
    // reach shared logs.
    let pg = PostgresStore::open(&url, kms.clone())
        .unwrap_or_else(|e| panic!("postgres+pgvector required (compose up): {e}"));
    let lite = open_sqlite();
    let bank = unique("bank-window-parity");
    let items = parity_fixture(&bank);
    for item in &items {
        pg.create_memory_item(item, "agent").expect("pg create");
        lite.create_memory_item(item, "agent")
            .expect("sqlite create");
    }
    // Archive the same row on both backends right after seeding.
    pg.archive_item(
        &bank,
        &format!("{bank}-archived"),
        "2024-03-25T00:00:00Z",
        "agent",
    )
    .expect("pg archive");
    lite.archive_item(
        &bank,
        &format!("{bank}-archived"),
        "2024-03-25T00:00:00Z",
        "agent",
    )
    .expect("sqlite archive");

    let valid_expected = vec![
        format!("{bank}-live"),
        format!("{bank}-closed"),
        format!("{bank}-open"),
    ];
    let ids_from_sqlite = lite
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 100)
        .expect("sqlite valid");
    let ids_from_postgres = pg
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 100)
        .expect("pg valid");
    assert_eq!(ids_from_sqlite, ids_from_postgres);
    assert_eq!(ids_from_postgres, valid_expected);
    // The open-ended item that started before the window stays a candidate.
    assert!(ids_from_postgres.contains(&format!("{bank}-open")));

    let tx_expected = vec![
        format!("{bank}-tx-inside"),
        format!("{bank}-tx-open"),
        format!("{bank}-late"),
        format!("{bank}-live"),
        format!("{bank}-closed"),
        format!("{bank}-open"),
    ];
    let tx_ids_from_sqlite = lite
        .item_ids_in_window(&bank, TimeAxis::Transaction, START, END, 100)
        .expect("sqlite tx");
    let tx_ids_from_postgres = pg
        .item_ids_in_window(&bank, TimeAxis::Transaction, START, END, 100)
        .expect("pg tx");
    assert_eq!(tx_ids_from_sqlite, tx_ids_from_postgres);
    assert_eq!(tx_ids_from_postgres, tx_expected);

    let capped_from_sqlite = lite
        .item_ids_in_window(&bank, TimeAxis::Transaction, START, END, 3)
        .expect("sqlite capped");
    let capped_from_postgres = pg
        .item_ids_in_window(&bank, TimeAxis::Transaction, START, END, 3)
        .expect("pg capped");
    assert_eq!(capped_from_sqlite, capped_from_postgres);
    assert_eq!(
        capped_from_postgres,
        vec![
            format!("{bank}-tx-inside"),
            format!("{bank}-tx-open"),
            format!("{bank}-late"),
        ]
    );

    for item in &items {
        pg.delete_item(&bank, &item.id).expect("delete pg item");
        lite.delete_item(&bank, &item.id)
            .expect("delete sqlite item");
    }
}
