// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite discard write's stored timestamps:
//! `updated_at` and `discarded_at` land in the canonical RFC 3339 form the item
//! time columns are compared in, never as epoch millis.
//!
//! ## Owns
//! - The stamp `discard_item` writes into `items`
//! - The identity read and the rejections that guard the write
//!
//! ## Does not own
//! - The canonical form itself (`migrate_canonical_tests`)
//! - Postgres discard stamps (`postgres_discard_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::migrate::{canonical_ts, is_epoch_stamp};
use crate::{LocalDevKms, SqliteStore, Store};
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
        gist: Some("discard stamp gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Read `updated_at` and `discarded_at` back for one row.
fn stamps(store: &SqliteStore, id: &str) -> (String, Option<String>) {
    store
        .conn
        .lock()
        .expect("store lock")
        .query_row(
            "SELECT updated_at, discarded_at FROM items WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("read stamps")
}

#[test]
fn discard_stores_the_canonical_updated_at() {
    let store = store();
    store
        .create_memory_item(&item("itm-discard", "bank"), "agent")
        .expect("seed");

    store
        .discard_item(
            "bank",
            "itm-discard",
            "ops cleanup",
            "2026-09-20T00:00:00Z",
            "agent",
        )
        .expect("discard");

    let (updated_at, discarded_at) = stamps(&store, "itm-discard");
    assert_eq!(
        canonical_ts(&updated_at),
        updated_at,
        "the writer's own stamp is already canonical: {updated_at}"
    );
    assert!(
        !is_epoch_stamp(&updated_at),
        "epoch millis sorts before every canonical stamp: {updated_at}"
    );
    assert_eq!(discarded_at.as_deref(), Some("2026-09-20T00:00:00Z"));
}

#[test]
fn the_identity_read_sees_the_live_row_and_a_repeat_discard_is_rejected() {
    let store = store();
    store
        .create_memory_item(&item("itm-twice", "bank"), "agent")
        .expect("seed");

    let identity = store
        .get_item_identity("bank", "itm-twice")
        .expect("identity read")
        .expect("row is live");
    assert_eq!(identity.item_id, "itm-twice");

    store
        .discard_item(
            "bank",
            "itm-twice",
            "ops cleanup",
            "2026-09-20T00:00:00Z",
            "agent",
        )
        .expect("first discard");
    assert!(
        store
            .get_item_identity("bank", "itm-twice")
            .expect("identity read")
            .is_none(),
        "a discarded row is not an identity any more"
    );
    assert_eq!(
        store
            .discard_item(
                "bank",
                "itm-twice",
                "again",
                "2026-09-21T00:00:00Z",
                "agent"
            )
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
}

#[test]
fn discard_refuses_a_missing_row_or_an_empty_argument() {
    let store = store();
    store
        .create_memory_item(&item("itm-args", "bank"), "agent")
        .expect("seed");

    assert_eq!(
        store
            .discard_item(
                "bank",
                "itm-gone",
                "ops cleanup",
                "2026-09-20T00:00:00Z",
                "agent"
            )
            .unwrap_err()
            .code,
        clio_types::ErrorCode::ItemNotFound
    );
    for (bank, item_id, reason, actor) in [
        ("", "itm-args", "ops cleanup", "agent"),
        ("bank", "", "ops cleanup", "agent"),
        ("bank", "itm-args", "", "agent"),
        ("bank", "itm-args", "ops cleanup", ""),
    ] {
        assert_eq!(
            store
                .discard_item(bank, item_id, reason, "2026-09-20T00:00:00Z", actor)
                .unwrap_err()
                .code,
            clio_types::ErrorCode::InvalidArgument,
            "{bank}/{item_id}/{reason}/{actor}"
        );
    }
}
