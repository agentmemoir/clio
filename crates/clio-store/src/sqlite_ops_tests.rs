// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite `OpsStore` SQL this module owns:
//! reachability, the recorded schema version, settings reads and writes, the
//! bank catalog, content-free counts, and bank-scoped orphan edge cleanup.
//!
//! ## Owns
//! - `setting_get` / `setting_set` behavior, including the upsert row count
//! - `ping`, `schema_version`, `bank_ids`, `bank_exists`, `register_bank`
//! - `counts`, `integrity_signals`, `drop_orphan_assoc_edges`
//! - The read-only `sqlite_store_counts` reader
//!
//! ## Does not own
//! - The contract definition (`ops_store`) or trigger coverage
//!   (`bank_registration_tests`)
//! - Postgres parity for the same contract (`postgres_ops_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the SQLite operator-health SQL changes; they never
//! change it.
//!
//! Keep this module focused on SQLite operator-health tests.

use super::sqlite_store_counts;
use crate::LocalDevKms;
use crate::migrate::{IntoAmError, SCHEMA_VERSION};
use crate::model::{AssocEdgeRecord, AssocKind};
use crate::ops_store::{IntegritySignals, OpsStore, StoreCounts};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;

fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("sqlite open")
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("ev".into()),
        snapshot: Some(json!({ "k": id })),
        gist: Some("ops gist".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn edge(id: &str, bank: &str, source: &str, target: &str) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: bank.into(),
        source_id: source.into(),
        target_id: target.into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("r".into()),
        weight: 0.5,
        last_updated_at: "t0".into(),
    }
}

/// The `schema_settings` rows stored for one key, read with raw SQL.
fn setting_rows(store: &SqliteStore, key: &str) -> Vec<(String, Option<String>)> {
    store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT value, updated_at FROM schema_settings WHERE key = ?1")
                .am("settings prepare")?;
            let rows = stmt
                .query_map(rusqlite::params![key], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                })
                .am("settings rows")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("settings row")?);
            }
            Ok(out)
        })
        .expect("settings query")
}

#[test]
fn setting_get_missing_key_is_none() {
    let store = store();
    assert!(store.setting_get("absent-setting").unwrap().is_none());
}

#[test]
fn setting_set_then_get_returns_the_value_exactly() {
    let store = store();
    let key = "tls-ca-path";
    let value = "/etc/clio/ca bundle.pem";
    store.setting_set(key, value).unwrap();
    assert_eq!(store.setting_get(key).unwrap().as_deref(), Some(value));
}

#[test]
fn setting_round_trips_a_device_id_shape() {
    let store = store();
    let key = "sync_device_id";
    let value = "dev_9f2c-1a4b_7d3e-0b6f";
    store.setting_set(key, value).unwrap();
    assert_eq!(store.setting_get(key).unwrap().as_deref(), Some(value));
}

#[test]
fn setting_set_twice_updates_one_row_without_error() {
    let store = store();
    let key = "overwrite-setting";
    store.setting_set(key, "first").unwrap();
    store.setting_set(key, "second").unwrap();

    let rows = setting_rows(&store, key);
    assert_eq!(rows.len(), 1, "an upsert must not duplicate the row");
    assert_eq!(rows[0].0, "second");
    assert!(
        rows[0].1.is_some(),
        "the upsert must stamp updated_at on the row"
    );
}

#[test]
fn setting_set_overwrites_a_seeded_row_and_keeps_created_at() {
    let store = store();
    let key = "seeded-setting";
    store
        .test_execute(
            "INSERT INTO schema_settings (key, value, created_at, updated_at)
             VALUES ('seeded-setting', 'old', '2020-01-01T00:00:00Z', NULL)",
        )
        .unwrap();
    store.setting_set(key, "new").unwrap();

    let created_at: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT created_at FROM schema_settings WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .am("created_at")
        })
        .unwrap();
    assert_eq!(created_at, "2020-01-01T00:00:00Z");
    assert_eq!(store.setting_get(key).unwrap().as_deref(), Some("new"));
}

#[test]
fn ping_succeeds_and_schema_version_reads_then_goes_absent() {
    let store = store();
    store.ping().expect("in-memory store answers ping");
    assert_eq!(
        store.schema_version().unwrap().as_deref(),
        Some(SCHEMA_VERSION)
    );
    store
        .test_execute("DELETE FROM schema_settings WHERE key = 'schema_version'")
        .unwrap();
    assert!(store.schema_version().unwrap().is_none());
}

#[test]
fn bank_ids_are_sorted_and_bank_exists_tracks_the_catalog() {
    let store = store();
    assert_eq!(store.bank_ids().unwrap(), Vec::<String>::new());
    assert!(!store.bank_exists("unknown-bank").unwrap());

    store
        .register_bank("bank-b", "2024-01-01T00:00:00Z")
        .unwrap();
    store
        .register_bank("bank-a", "2024-01-01T00:00:00Z")
        .unwrap();
    assert_eq!(
        store.bank_ids().unwrap(),
        vec!["bank-a".to_owned(), "bank-b".to_owned()]
    );
    assert!(store.bank_exists("bank-a").unwrap());
    assert!(!store.bank_exists("bank-c").unwrap());
}

#[test]
fn register_bank_twice_keeps_one_row_and_the_first_created_at() {
    let store = store();
    store
        .register_bank("twice-bank", "2024-01-01T00:00:00Z")
        .unwrap();
    store
        .register_bank("twice-bank", "2025-01-01T00:00:00Z")
        .unwrap();

    let created: Vec<String> = store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT created_at FROM banks WHERE id = 'twice-bank'")
                .am("banks prepare")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .am("banks rows")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("banks row")?);
            }
            Ok(out)
        })
        .unwrap();
    assert_eq!(created, vec!["2024-01-01T00:00:00Z".to_owned()]);
}

#[test]
fn a_memory_write_registers_its_bank() {
    let store = store();
    store
        .create_memory_item(&item("reg-1", "written-bank"), "actor")
        .unwrap();
    assert!(store.bank_exists("written-bank").unwrap());
    assert!(
        store
            .bank_ids()
            .unwrap()
            .contains(&"written-bank".to_owned())
    );
}

#[test]
fn counts_report_memories_and_distinct_subjects() {
    let store = store();
    assert_eq!(store.counts().unwrap(), StoreCounts::default());

    let mut first = item("counts-1", "counts");
    first.subject_id = "subject-a".to_owned();
    let mut second = item("counts-2", "counts");
    second.subject_id = "subject-a".to_owned();
    let mut third = item("counts-3", "counts");
    third.subject_id = "subject-b".to_owned();
    for record in [&first, &second, &third] {
        store.create_memory_item(record, "actor").unwrap();
    }

    assert_eq!(
        store.counts().unwrap(),
        StoreCounts {
            memories: 3,
            subjects: 2
        }
    );
}

#[test]
fn embedding_space_matches_the_store_space() {
    let store = store();
    let space = OpsStore::embedding_space(&store).unwrap();
    assert_eq!(space, *store.space());
    assert_eq!(space.dims(), crate::model::EMBEDDING_DIMS);
}

#[test]
fn integrity_signals_are_empty_and_then_bank_scoped() {
    let store = store();
    let bank_a = "ops-a";
    let bank_b = "ops-b";
    assert_eq!(
        store.integrity_signals(None).unwrap(),
        IntegritySignals::default()
    );

    store
        .create_memory_item(&item("live-a", bank_a), "actor")
        .unwrap();
    store
        .put_assoc_edge(&edge("orphan-edge", bank_a, "ghost", "live-a"))
        .unwrap();
    store
        .test_execute(&format!(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext,
                                  epistemic_kind, valid_from, tx_from, created_at)
             VALUES ('orphan-triple', '{bank_a}', 'missing-item', 's', 'p',
                     'sealed-fixture', 'fact', 't0', 't0', 't0');
             PRAGMA foreign_keys = ON"
        ))
        .unwrap();
    store
        .test_execute(&format!(
            "INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext,
                                  epistemic_kind, valid_from, tx_from, snapshot_ref,
                                  gist_ref, created_at)
             VALUES ('dangling-triple', '{bank_a}', 'live-a', 's', 'p2',
                     'sealed-fixture', 'fact', 't0', 't0', 'missing-snap',
                     'missing-gist', 't0')"
        ))
        .unwrap();
    store
        .test_execute(&format!(
            "INSERT INTO memtree_nodes (id, bank_id, item_id, depth, interval_start,
                                        interval_end, child_ids_json, dirty, generation,
                                        refreshing, created_at, updated_at)
             VALUES ('orphan-leaf', '{bank_a}', 'missing-item', 0, 't0', 't0', '[]',
                     0, 0, 0, 't0', 't0')"
        ))
        .unwrap();

    let a = store.integrity_signals(Some(bank_a)).unwrap();
    assert_eq!(a.orphan_assoc_edges, 1);
    assert_eq!(a.orphan_triple_items, 1);
    assert_eq!(a.dangling_snapshot_refs, 1);
    assert_eq!(a.dangling_gist_refs, 1);
    assert_eq!(a.orphan_memtree_leaf_items, 1);

    assert_eq!(
        store.integrity_signals(Some(bank_b)).unwrap(),
        IntegritySignals::default(),
        "another bank's orphans must not be counted"
    );
}

#[test]
fn drop_orphan_assoc_edges_keeps_live_edges_and_rejects_an_empty_bank() {
    let store = store();
    let bank = "ops-drop";
    store
        .create_memory_item(&item("live-1", bank), "actor")
        .unwrap();
    store
        .create_memory_item(&item("live-2", bank), "actor")
        .unwrap();
    store
        .put_assoc_edge(&edge("orphan", bank, "ghost", "live-1"))
        .unwrap();
    store
        .put_assoc_edge(&edge("live", bank, "live-1", "live-2"))
        .unwrap();

    assert_eq!(
        store.drop_orphan_assoc_edges("").unwrap_err().code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        store.drop_orphan_assoc_edges("   ").unwrap_err().code,
        ErrorCode::MissingBank
    );

    assert_eq!(store.drop_orphan_assoc_edges(bank).unwrap(), 1);
    assert!(store.get_assoc_edge(bank, "live").unwrap().is_some());
    assert!(store.get_assoc_edge(bank, "orphan").unwrap().is_none());
    assert_eq!(
        store
            .integrity_signals(Some(bank))
            .unwrap()
            .orphan_assoc_edges,
        0
    );
}

#[test]
fn read_only_counts_leave_the_file_unchanged_and_refuse_a_non_store() {
    let dir = std::env::temp_dir().join(unique("clio-ops-tests"));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("counts.db");

    let store = SqliteStore::open(&path, Arc::new(LocalDevKms::new())).expect("open");
    let mut first = item("ro-1", "ro");
    first.subject_id = "subject-a".to_owned();
    let mut second = item("ro-2", "ro");
    second.subject_id = "subject-b".to_owned();
    store.create_memory_item(&first, "actor").unwrap();
    store.create_memory_item(&second, "actor").unwrap();
    drop(store);

    let before = std::fs::read(&path).expect("read store");
    assert_eq!(
        sqlite_store_counts(&path).expect("read-only counts"),
        StoreCounts {
            memories: 2,
            subjects: 2
        }
    );
    assert_eq!(
        std::fs::read(&path).expect("read store"),
        before,
        "a read-only count must not write to the file"
    );

    let other = dir.join("other.db");
    std::fs::write(&other, b"not sqlite").expect("write other");
    assert!(sqlite_store_counts(&other).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}
