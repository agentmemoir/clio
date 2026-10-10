// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests hygiene persistence on Postgres: flag /
//! archive / unarchive roundtrip, the retrieve-eligibility probe, and the
//! durable hygiene log, on the shared Compose `clio` database.
//!
//! ## Owns
//! - Postgres parity coverage for the `HygieneStore` contract
//!
//! ## Does not own
//! - SQLite-path hygiene behavior (`clio-hygiene` crate tests)
//! - The store SQL itself (`sqlite_hygiene` / `postgres_hygiene`)
//!
//! ## Boundary
//!
//! Tests here break when Postgres hygiene SQL changes; they never change it.
//!
//! Keep this module focused on Postgres hygiene parity tests.

use crate::db_url::postgres_url_from_env;
use crate::{
    DekProvider, HygieneLogEntry, HygieneStore, LocalDevKms, PostgresStore, SqliteStore, Store,
};
use clio_types::MemoryItem;
use clio_types::{EpistemicKind, ItemKind, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

use crate::test_unique::unique;

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
        gist: Some("hygiene parity gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Shared parity body: one bank, one seeded item, full hygiene roundtrip.
fn parity_body<S: Store + HygieneStore>(s: &S, bank: &str, id: &str) {
    s.create_memory_item(&item(id, bank), "agent")
        .expect("seed");

    // Archive: sets the marker, emits telemetry, rejects repeats.
    let identity = s
        .archive_item(bank, id, "2026-09-20T00:00:00Z", "agent")
        .expect("archive");
    assert_eq!(identity.item_id, id);
    assert!(s.item_is_archived(bank, id).expect("probe"));
    assert_eq!(
        s.archive_item(bank, id, "2026-09-20T00:00:00Z", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
    // Telemetry row inside the archive transaction.
    let trail = s.audit_trail(bank, id).expect("trail");
    assert!(trail.events.iter().any(|e| e.operation == "archive"));

    // Unarchive restores; a second unarchive is rejected.
    s.unarchive_item(bank, id, "agent").expect("unarchive");
    assert!(!s.item_is_archived(bank, id).expect("probe"));
    assert_eq!(
        s.unarchive_item(bank, id, "agent").unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );

    // Flag set/clear roundtrip.
    s.set_hygiene_flag(bank, id, Some("review"), "agent", "2026-09-20T00:00:00Z")
        .expect("flag");
    s.set_hygiene_flag(bank, id, None, "agent", "2026-09-20T00:00:00Z")
        .expect("clear flag");

    // Durable log roundtrip, bank-scoped, newest-first.
    let entry = HygieneLogEntry {
        id: format!("hlg-{bank}-{id}"),
        bank_id: bank.to_owned(),
        item_id: id.to_owned(),
        noise_score: Some(0.91),
        action: "archive".into(),
        actor: "agent".into(),
        confirmed: true,
        dry_run: false,
        detail: json!({ "result": "applied", "batch_id": "pg-batch" }),
        created_at: "2026-09-20T00:00:00Z".into(),
    };
    s.insert_hygiene_log(&entry).expect("log insert");
    let rows = s.list_hygiene_logs(Some(bank), 50, 0).expect("log list");
    assert!(rows.iter().any(|r| r.id == entry.id));
    assert!(rows[0].created_at >= rows[rows.len() - 1].created_at);
    // Cross-bank listing works; pagination bounds hold.
    let all_banks = s.list_hygiene_logs(None, 200, 0).expect("all banks");
    assert!(!all_banks.is_empty(), "{all_banks:?}");
    assert!(s.list_hygiene_logs(Some(bank), 1, 0).is_ok());
    assert_eq!(
        s.list_hygiene_logs(Some(bank), 0, 0).unwrap_err().code,
        clio_types::ErrorCode::OutOfRange
    );

    // Invalid log action fails closed on both backends.
    let bad = HygieneLogEntry {
        action: "erase_request".into(),
        ..entry
    };
    assert_eq!(
        s.insert_hygiene_log(&bad).unwrap_err().code,
        clio_types::ErrorCode::InvalidArgument
    );

    // Unknown items are ItemNotFound; empty ids are InvalidArgument.
    assert_eq!(
        s.archive_item(bank, &unique("ghost"), "2026-09-20T00:00:00Z", "agent")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::ItemNotFound
    );
    assert_eq!(
        s.set_hygiene_flag("", id, Some("x"), "agent", "2026-09-20T00:00:00Z")
            .unwrap_err()
            .code,
        clio_types::ErrorCode::InvalidArgument
    );
}

#[test]
fn hygiene_store_parity() {
    let _guard = crate::lock_pg_test();
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = PostgresStore::open(url.as_str(), kms).expect("pg open");
    let lite = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("sqlite open");
    let pg_bank = unique("bank-hyg-pg");
    let pg_id = unique("item-pg");
    parity_body(&pg, &pg_bank, &pg_id);
    let lite_bank = unique("bank-hyg-lite");
    let lite_id = unique("item-lite");
    parity_body(&lite, &lite_bank, &lite_id);
}
