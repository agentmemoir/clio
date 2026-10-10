// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the SQLite item-window query and the
//! shared window compiler both backends use.
//!
//! ## Owns
//! - SQLite membership, ordering, eligibility, isolation, and validation tests
//! - The fixture helpers the Postgres parity test reuses
//!
//! ## Does not own
//! - Postgres execution parity (see `pg_window_tests`)
//! - Production SQL (see `window_query` and `sqlite_window`)
//!
//! ## Boundary
//!
//! Changes to SQLite window behavior should normally update tests here.
//! Changes to Postgres execution should not require changes here.
//!
//! Keep this module focused on SQLite window behavior and shared fixtures.

use crate::sqlite::SqliteStore;
use crate::window_query::MAX_WINDOW_ITEMS;
use crate::window_query::compile_window;
use crate::{DekProvider, HygieneStore, LocalDevKms, Store};
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory, TimeAxis};
use serde_json::json;
use std::sync::Arc;

/// Window every membership test runs against.
pub(crate) const START: &str = "2024-03-01T00:00:00Z";
/// Exclusive window end every membership test runs against.
pub(crate) const END: &str = "2024-04-01T00:00:00Z";

/// Unique suffix so parallel and repeated runs never share rows.
use crate::test_unique::unique;

/// One fact item with explicit validity and transaction intervals.
pub(crate) fn fixture(
    id: &str,
    bank: &str,
    valid_from: &str,
    valid_until: Option<&str>,
    tx_from: &str,
    tx_until: Option<&str>,
) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("{bank}-subject"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.5),
        admission_score: Some(0.5),
        source_ref: Some("window-test".to_owned()),
        snapshot: Some(json!({ "probe": id })),
        gist: Some("window fixture gist".to_owned()),
        valid_from: valid_from.to_owned(),
        valid_until: valid_until.map(str::to_owned),
        tx_from: tx_from.to_owned(),
        tx_until: tx_until.map(str::to_owned),
        created_at: "2024-01-01T00:00:00Z".to_owned(),
        updated_at: "2024-01-01T00:00:00Z".to_owned(),
        context: None,
    }
}

/// In-memory SQLite store with the default embedding space.
pub(crate) fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

fn create(store: &SqliteStore, item: &MemoryItem) {
    store
        .create_memory_item(item, "agent")
        .expect("create item");
}

#[test]
fn valid_axis_matches_half_open_overlap() {
    let store = open_sqlite();
    let bank = unique("bank-window");
    let rows: [(&str, &str, Option<&str>); 7] = [
        (
            "inside",
            "2024-03-10T00:00:00Z",
            Some("2024-03-20T00:00:00Z"),
        ),
        ("open", "2024-01-15T00:00:00Z", None),
        (
            "closed",
            "2024-03-05T00:00:00Z",
            Some("2024-03-25T00:00:00Z"),
        ),
        ("at-start", START, None),
        ("late", END, None),
        ("closed-at-start", "2024-02-01T00:00:00Z", Some(START)),
        (
            "before",
            "2024-01-01T00:00:00Z",
            Some("2024-02-01T00:00:00Z"),
        ),
    ];
    for (suffix, valid_from, valid_until) in rows {
        create(
            &store,
            &fixture(
                &format!("{bank}-{suffix}"),
                &bank,
                valid_from,
                valid_until,
                "2024-01-01T00:00:00Z",
                None,
            ),
        );
    }
    let got = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 100)
        .expect("window");
    // `late` starts exactly at the exclusive end, `closed-at-start` ends
    // exactly at the inclusive start, and `before` misses the window; only the
    // four overlapping items remain, start descending.
    assert_eq!(
        got,
        vec![
            format!("{bank}-inside"),
            format!("{bank}-closed"),
            format!("{bank}-at-start"),
            format!("{bank}-open"),
        ]
    );
}

#[test]
fn transaction_axis_uses_tx_columns() {
    let store = open_sqlite();
    let bank = unique("bank-tx-window");
    let rows: [(&str, &str, Option<&str>); 5] = [
        ("inside", "2024-03-15T00:00:00Z", None),
        ("before", "2024-01-01T00:00:00Z", None),
        ("at-start", START, None),
        ("late", END, None),
        (
            "superseded",
            "2024-03-15T00:00:00Z",
            Some("2024-03-20T00:00:00Z"),
        ),
    ];
    // The validity columns sit far outside the window, so only the
    // transaction columns can decide membership here.
    for (suffix, tx_from, tx_until) in rows {
        create(
            &store,
            &fixture(
                &format!("{bank}-{suffix}"),
                &bank,
                "2020-01-01T00:00:00Z",
                Some("2020-02-01T00:00:00Z"),
                tx_from,
                tx_until,
            ),
        );
    }
    let got = store
        .item_ids_in_window(&bank, TimeAxis::Transaction, START, END, 100)
        .expect("window");
    assert_eq!(
        got,
        vec![
            format!("{bank}-inside"),
            format!("{bank}-at-start"),
            format!("{bank}-before"),
        ]
    );
}

#[test]
fn ordering_is_start_desc_then_id_asc_and_limit_truncates() {
    let store = open_sqlite();
    let bank = unique("bank-order");
    let a = format!("{bank}-a");
    let b = format!("{bank}-b");
    let c = format!("{bank}-c");
    for (id, valid_from) in [
        (&b, "2024-03-05T00:00:00Z"),
        (&a, "2024-03-05T00:00:00Z"),
        (&c, "2024-03-10T00:00:00Z"),
    ] {
        create(
            &store,
            &fixture(id, &bank, valid_from, None, "2024-01-01T00:00:00Z", None),
        );
    }
    let all = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 100)
        .expect("window");
    // `c` starts latest; `a` and `b` tie, so the id breaks the tie ascending.
    assert_eq!(all, vec![c.clone(), a.clone(), b]);
    let capped = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 2)
        .expect("window");
    assert_eq!(capped, vec![c, a]);
}

#[test]
fn discarded_and_superseded_items_are_not_candidates() {
    let store = open_sqlite();
    let bank = unique("bank-elig");
    let active = format!("{bank}-active");
    let discarded = format!("{bank}-discarded");
    create(
        &store,
        &fixture(
            &active,
            &bank,
            "2024-03-10T00:00:00Z",
            None,
            "2024-01-01T00:00:00Z",
            None,
        ),
    );
    create(
        &store,
        &fixture(
            &discarded,
            &bank,
            "2024-03-10T00:00:00Z",
            None,
            "2024-01-01T00:00:00Z",
            None,
        ),
    );
    store
        .discard_item(
            &bank,
            &discarded,
            "test discard",
            "2024-03-15T00:00:00Z",
            "agent",
        )
        .expect("discard");
    create(
        &store,
        &fixture(
            &format!("{bank}-superseded"),
            &bank,
            "2024-03-20T00:00:00Z",
            None,
            "2024-01-01T00:00:00Z",
            Some("2024-03-25T00:00:00Z"),
        ),
    );
    let got = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 100)
        .expect("window");
    assert_eq!(got, vec![active]);
}

#[test]
fn archived_items_are_not_candidates_and_do_not_consume_limit() {
    let store = open_sqlite();
    let bank = unique("bank-archive");
    // Three in-window, open-ended items, newest valid_from first.
    let rows: [(&str, &str); 3] = [
        ("newest", "2024-03-20T00:00:00Z"),
        ("middle", "2024-03-10T00:00:00Z"),
        ("oldest", "2024-03-05T00:00:00Z"),
    ];
    for (suffix, valid_from) in rows {
        create(
            &store,
            &fixture(
                &format!("{bank}-{suffix}"),
                &bank,
                valid_from,
                None,
                "2024-01-01T00:00:00Z",
                None,
            ),
        );
    }
    let newest = format!("{bank}-newest");
    store
        .archive_item(&bank, &newest, "2024-03-25T00:00:00Z", "agent")
        .expect("archive");
    let live = vec![format!("{bank}-middle"), format!("{bank}-oldest")];
    // The archived row is absent from the results entirely.
    let all = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 100)
        .expect("window");
    assert_eq!(all, live);
    // A limit smaller than the seeded count still returns both live items,
    // proving the archived row never consumed a LIMIT slot.
    let capped = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 2)
        .expect("window");
    assert_eq!(capped, live);
}

#[test]
fn windows_never_cross_banks() {
    let store = open_sqlite();
    let bank_a = unique("bank-a");
    let bank_b = unique("bank-b");
    let item_a = format!("{bank_a}-item");
    let item_b = format!("{bank_b}-item");
    for (bank, id) in [(&bank_a, &item_a), (&bank_b, &item_b)] {
        create(
            &store,
            &fixture(
                id,
                bank,
                "2024-03-10T00:00:00Z",
                None,
                "2024-01-01T00:00:00Z",
                None,
            ),
        );
    }
    assert_eq!(
        store
            .item_ids_in_window(&bank_a, TimeAxis::Valid, START, END, 100)
            .expect("bank a"),
        vec![item_a]
    );
    assert_eq!(
        store
            .item_ids_in_window(&bank_b, TimeAxis::Valid, START, END, 100)
            .expect("bank b"),
        vec![item_b]
    );
}

#[test]
fn window_validation_errors() {
    let store = open_sqlite();
    let bank = unique("bank-bad");

    let err = store
        .item_ids_in_window("", TimeAxis::Valid, START, END, 10)
        .expect_err("empty bank");
    assert_eq!(err.code, ErrorCode::MissingBank);
    assert_eq!(err.message, "item window requires a non-empty bank");

    let err = store
        .item_ids_in_window(&bank, TimeAxis::Valid, "", END, 10)
        .expect_err("empty start");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, "item window bounds must be non-empty");

    let err = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, "", 10)
        .expect_err("empty end");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let err = store
        .item_ids_in_window(&bank, TimeAxis::Valid, END, START, 10)
        .expect_err("reversed bounds");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, "item window start must be before its end");

    let err = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, START, 10)
        .expect_err("equal bounds");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    let err = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 0)
        .expect_err("zero limit");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, "item window limit must be positive");

    let err = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, MAX_WINDOW_ITEMS + 1)
        .expect_err("limit over cap");
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert_eq!(
        err.message,
        format!(
            "item window limit is capped at {MAX_WINDOW_ITEMS}, got {}",
            MAX_WINDOW_ITEMS + 1
        )
    );

    // The cap itself is valid input; this bank simply has no rows.
    let capped = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, MAX_WINDOW_ITEMS)
        .expect("cap allowed");
    assert!(capped.is_empty(), "{capped:?}");
}

#[test]
fn compiler_emits_expected_sql_for_both_placeholders() {
    let compiled = compile_window("bank-a", TimeAxis::Valid, START, END, 7, '?').expect("compile");
    assert_eq!(
        compiled.sql,
        "SELECT id FROM items WHERE bank_id = ?1 AND discarded_at IS NULL AND archived_at IS NULL \
         AND tx_until IS NULL AND valid_from < ?2 AND (valid_until IS NULL OR valid_until > ?3) \
         ORDER BY valid_from DESC, id ASC LIMIT ?4"
    );
    assert_eq!(compiled.params, ["bank-a", END, START]);

    let compiled =
        compile_window("bank-a", TimeAxis::Transaction, START, END, 7, '$').expect("compile");
    assert_eq!(
        compiled.sql,
        "SELECT id FROM items WHERE bank_id = $1 AND discarded_at IS NULL AND archived_at IS NULL \
         AND tx_until IS NULL AND tx_from < $2 AND (tx_until IS NULL OR tx_until > $3) \
         ORDER BY tx_from DESC, id ASC LIMIT $4"
    );
    assert_eq!(compiled.params, ["bank-a", END, START]);
}

#[test]
fn window_returns_item_ids_only() {
    let store = open_sqlite();
    let bank = unique("bank-ids");
    let id = format!("{bank}-item");
    create(
        &store,
        &fixture(
            &id,
            &bank,
            "2024-03-10T00:00:00Z",
            None,
            "2024-01-01T00:00:00Z",
            None,
        ),
    );
    let got = store
        .item_ids_in_window(&bank, TimeAxis::Valid, START, END, 10)
        .expect("window");
    // The result is exactly the matching item id: no content or scores.
    assert_eq!(got, vec![id]);
}
