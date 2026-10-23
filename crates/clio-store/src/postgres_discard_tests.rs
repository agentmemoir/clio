// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres discard write's stored
//! timestamps: `updated_at` and `discarded_at` land in the canonical RFC 3339
//! form the item time columns are compared in, never as epoch millis.
//!
//! ## Owns
//! - The stamp `discard_item` writes into `items` on Postgres
//! - The identity read and the rejections that guard the write
//!
//! ## Does not own
//! - The canonical form itself (`migrate_canonical_tests`)
//! - SQLite discard stamps (`sqlite_discard_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::migrate::{IntoAmError, canonical_ts, is_epoch_stamp};
use crate::test_unique::unique;
use crate::{DekProvider, LocalDevKms, PostgresStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

/// A connection-scoped Postgres store on the shared Compose database.
fn store() -> (crate::PgTestGuard, PostgresStore) {
    let guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    (
        guard,
        PostgresStore::open(url.as_str(), kms).expect("pg open"),
    )
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
fn stamps(store: &PostgresStore, bank: &str, id: &str) -> (String, Option<String>) {
    store
        .with_client(|client| {
            let row = client
                .query_one(
                    "SELECT updated_at, discarded_at FROM items WHERE id = $1 AND bank_id = $2",
                    &[&id, &bank],
                )
                .am("read stamps")?;
            Ok((row.get(0), row.get(1)))
        })
        .expect("read stamps")
}

#[test]
fn discard_stores_the_canonical_updated_at() {
    let (_guard, store) = store();
    let bank = unique("bank-pg-discard");
    let id = unique("item-pg-discard");
    store
        .create_memory_item(&item(&id, &bank), "agent")
        .expect("seed");

    store
        .discard_item(&bank, &id, "ops cleanup", "2026-09-20T00:00:00Z", "agent")
        .expect("discard");

    let (updated_at, discarded_at) = stamps(&store, &bank, &id);
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
    let (_guard, store) = store();
    let bank = unique("bank-pg-discard2");
    let id = unique("item-pg-discard2");
    store
        .create_memory_item(&item(&id, &bank), "agent")
        .expect("seed");

    assert!(
        store
            .get_item_identity(&bank, &id)
            .expect("identity read")
            .is_some()
    );
    store
        .discard_item(&bank, &id, "ops cleanup", "2026-09-20T00:00:00Z", "agent")
        .expect("first discard");
    assert!(
        store
            .get_item_identity(&bank, &id)
            .expect("identity read")
            .is_none()
    );
    assert_eq!(
        store
            .discard_item(&bank, &id, "again", "2026-09-21T00:00:00Z", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
}

#[test]
fn discard_refuses_a_missing_row_or_an_empty_argument() {
    let (_guard, store) = store();
    let bank = unique("bank-pg-discard3");
    let id = unique("item-pg-discard3");
    store
        .create_memory_item(&item(&id, &bank), "agent")
        .expect("seed");

    assert_eq!(
        store
            .discard_item(
                &bank,
                &unique("item-pg-gone"),
                "ops cleanup",
                "2026-09-20T00:00:00Z",
                "agent"
            )
            .unwrap_err()
            .code,
        clio_types::ErrorCode::ItemNotFound
    );
    for (bank_arg, item_arg, reason, actor) in [
        ("", id.as_str(), "ops cleanup", "agent"),
        (bank.as_str(), "", "ops cleanup", "agent"),
        (bank.as_str(), id.as_str(), "", "agent"),
        (bank.as_str(), id.as_str(), "ops cleanup", ""),
    ] {
        assert_eq!(
            store
                .discard_item(bank_arg, item_arg, reason, "2026-09-20T00:00:00Z", actor)
                .unwrap_err()
                .code,
            clio_types::ErrorCode::InvalidArgument,
            "{bank_arg}/{item_arg}/{reason}/{actor}"
        );
    }
}
