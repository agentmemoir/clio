// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite hygiene persistence: the flag,
//! archive and unarchive roundtrip, the eligibility probe, and the durable
//! hygiene log, with the item stamp the writer owns asserted to be canonical.
//!
//! ## Owns
//! - SQLite coverage for the `HygieneStore` contract and its stored stamps
//!
//! ## Does not own
//! - Postgres hygiene parity (`postgres_hygiene_tests`)
//! - The canonical form itself (`migrate_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::migrate::{canonical_ts, is_epoch_stamp};
use crate::{HygieneLogEntry, HygieneStore, LocalDevKms, SqliteStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

/// An in-memory store this test owns outright.
fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("open in-memory store")
}

/// One seedable item; every stamp is canonical, so only the writer's own stamp
/// can be the value under test.
fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("ev".into()),
        snapshot: Some(json!({ "k": id })),
        gist: Some("hygiene stamp gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Read the columns one hygiene write owns.
fn stamps(store: &SqliteStore, id: &str) -> (String, Option<String>, Option<String>) {
    store
        .conn
        .lock()
        .expect("store lock")
        .query_row(
            "SELECT updated_at, archived_at, hygiene_flag FROM items WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("read stamps")
}

/// The log entry every log test writes.
fn log_entry(bank: &str, id: &str) -> HygieneLogEntry {
    HygieneLogEntry {
        id: format!("hlg-{id}"),
        bank_id: bank.to_owned(),
        item_id: id.to_owned(),
        noise_score: Some(0.91),
        action: "archive".into(),
        actor: "agent".into(),
        confirmed: true,
        dry_run: false,
        detail: json!({ "result": "applied" }),
        created_at: "2026-09-20T00:00:00Z".into(),
    }
}

#[test]
fn archive_and_unarchive_write_the_canonical_updated_at() {
    let store = store();
    store
        .create_memory_item(&item("itm-hyg", "bank"), "agent")
        .expect("seed");

    store
        .archive_item("bank", "itm-hyg", "2026-09-20T00:00:00Z", "agent")
        .expect("archive");

    let (updated_at, archived_at, _) = stamps(&store, "itm-hyg");
    assert_eq!(
        canonical_ts(&updated_at),
        updated_at,
        "the archive stamp is already canonical: {updated_at}"
    );
    assert!(
        !is_epoch_stamp(&updated_at),
        "epoch millis sorts before every canonical stamp: {updated_at}"
    );
    assert_eq!(archived_at.as_deref(), Some("2026-09-20T00:00:00Z"));
    assert!(store.item_is_archived("bank", "itm-hyg").expect("probe"));

    store
        .unarchive_item("bank", "itm-hyg", "agent")
        .expect("unarchive");

    let (unarchived_at, archived_at, _) = stamps(&store, "itm-hyg");
    assert_eq!(canonical_ts(&unarchived_at), unarchived_at);
    assert!(!is_epoch_stamp(&unarchived_at));
    assert_eq!(archived_at, None);
    assert!(!store.item_is_archived("bank", "itm-hyg").expect("probe"));
}

#[test]
fn the_flag_roundtrip_sets_and_clears_the_note() {
    let store = store();
    store
        .create_memory_item(&item("itm-flag", "bank"), "agent")
        .expect("seed");

    store
        .set_hygiene_flag(
            "bank",
            "itm-flag",
            Some("review"),
            "agent",
            "2026-09-20T00:00:00Z",
        )
        .expect("flag");
    assert_eq!(stamps(&store, "itm-flag").2.as_deref(), Some("review"));

    store
        .set_hygiene_flag("bank", "itm-flag", None, "agent", "2026-09-20T00:00:00Z")
        .expect("clear flag");
    assert_eq!(stamps(&store, "itm-flag").2, None);
}

#[test]
fn a_repeated_or_unknown_action_is_rejected() {
    let store = store();
    store
        .create_memory_item(&item("itm-rep", "bank"), "agent")
        .expect("seed");
    store
        .archive_item("bank", "itm-rep", "2026-09-20T00:00:00Z", "agent")
        .expect("archive");

    assert_eq!(
        store
            .archive_item("bank", "itm-rep", "2026-09-20T00:00:00Z", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .unarchive_item("bank", "itm-missing", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::ItemNotFound
    );
    assert_eq!(
        store
            .set_hygiene_flag("", "itm-rep", Some("x"), "agent", "2026-09-20T00:00:00Z")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    assert_eq!(
        store
            .unarchive_item("bank", "itm-rep", "")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
}

#[test]
fn the_hygiene_log_roundtrips_and_bounds_its_page() {
    let store = store();
    let entry = log_entry("bank", "itm-log");
    store.insert_hygiene_log(&entry).expect("log insert");

    let rows = store.list_hygiene_logs(Some("bank"), 50, 0).expect("list");
    assert!(rows.iter().any(|r| r.id == entry.id), "{rows:?}");
    let all = store.list_hygiene_logs(None, 200, 0).expect("all");
    assert!(!all.is_empty(), "the unscoped listing is empty");
    assert!(store.list_hygiene_logs(Some("bank"), 1, 0).is_ok());
    assert_eq!(
        store
            .list_hygiene_logs(Some("bank"), 0, 0)
            .unwrap_err()
            .code,
        clio_types::ErrorCode::OutOfRange
    );
    assert_eq!(
        store
            .list_hygiene_logs(Some("bank"), 201, 0)
            .unwrap_err()
            .code,
        clio_types::ErrorCode::OutOfRange
    );
    let bad = HygieneLogEntry {
        action: "erase_request".into(),
        ..entry
    };
    assert_eq!(
        store.insert_hygiene_log(&bad).unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );
}
