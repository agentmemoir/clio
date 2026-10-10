// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the SQLite `OpsStore` operator-health
//! contract: reachability, recorded schema version, bank inventory, content-free
//! referential-integrity counts, and bank-scoped orphan edge cleanup.
//!
//! ## Owns
//! - SQLite-backed `OpsStore` behavior, including planted-orphan counts and
//!   bank scoping of every integrity signal
//!
//! ## Does not own
//! - The contract definition (`ops_store`) or the SQL under test (`sqlite_ops`)
//! - Postgres parity for the same contract (`pg_ops_tests`)
//!
//! ## Boundary
//!
//! Tests here break when SQLite operator-health SQL changes; they never change it.
//!
//! Keep this module focused on SQLite operator-health tests.

use crate::LocalDevKms;
use crate::model::{AssocEdgeRecord, AssocKind};
use crate::ops_store::{IntegritySignals, OpsStore, StoreCounts};
use crate::sqlite::SqliteStore;
use crate::store::Store;
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

#[test]
fn sqlite_ping_and_schema_version() {
    let store = store();
    assert!(store.ping().is_ok());
    assert_eq!(store.schema_version().unwrap().as_deref(), Some("1"));
    store
        .test_execute("DELETE FROM schema_settings WHERE key = 'schema_version'")
        .unwrap();
    assert!(store.schema_version().unwrap().is_none());
}

#[test]
fn sqlite_bank_ids_sorted_and_empty() {
    let store = store();
    let banks = store.bank_ids().unwrap();
    assert!(banks.is_empty(), "{banks:?}");
    store
        .create_memory_item(&item("i-b", "bank-b"), "actor")
        .unwrap();
    store
        .create_memory_item(&item("i-a", "bank-a"), "actor")
        .unwrap();
    assert_eq!(
        store.bank_ids().unwrap(),
        vec!["bank-a".to_owned(), "bank-b".to_owned()]
    );
}

#[test]
fn sqlite_counts_memories_and_distinct_subjects() {
    let store = store();
    assert_eq!(store.counts().unwrap(), StoreCounts::default());

    let mut first = item("counts-1", "counts");
    first.subject_id = "subject-a".to_owned();
    let mut second = item("counts-2", "counts");
    second.subject_id = "subject-a".to_owned();
    let mut third = item("counts-3", "counts");
    third.subject_id = "subject-b".to_owned();
    store.create_memory_item(&first, "actor").unwrap();
    store.create_memory_item(&second, "actor").unwrap();
    store.create_memory_item(&third, "actor").unwrap();

    assert_eq!(
        store.counts().unwrap(),
        StoreCounts {
            memories: 3,
            subjects: 2
        }
    );
}

#[test]
fn sqlite_read_only_counts_leave_the_file_byte_identical() {
    let dir = std::env::temp_dir().join(format!("clio-ops-counts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("counts.db");
    let _ = std::fs::remove_file(&path);

    let store = SqliteStore::open(&path, Arc::new(LocalDevKms::new())).expect("open");
    let mut first = item("ro-1", "ro");
    first.subject_id = "subject-a".to_owned();
    let mut second = item("ro-2", "ro");
    second.subject_id = "subject-b".to_owned();
    store.create_memory_item(&first, "actor").unwrap();
    store.create_memory_item(&second, "actor").unwrap();
    drop(store);

    let before = std::fs::read(&path).expect("read");
    let counts = crate::sqlite_store_counts(&path).expect("read-only counts");
    assert_eq!(
        counts,
        StoreCounts {
            memories: 2,
            subjects: 2
        }
    );
    assert_eq!(
        std::fs::read(&path).expect("read"),
        before,
        "a read-only count must not write to the file"
    );

    // A file that is not a Clio store fails instead of reporting zeroes.
    let other = dir.join("other.db");
    std::fs::write(&other, b"not sqlite").expect("write");
    assert!(crate::sqlite_store_counts(&other).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sqlite_integrity_signals_clean_then_orphans_scoped() {
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
             VALUES ('orphan-triple', '{bank_a}', 'missing-item', 's', 'p', 'sealed-fixture',
                     'fact', 't0', 't0', 't0');
             PRAGMA foreign_keys = ON"
        ))
        .unwrap();
    store
        .test_execute(&format!(
            "INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext,
                                  epistemic_kind, valid_from, tx_from, snapshot_ref,
                                  gist_ref, created_at)
             VALUES ('dangling-triple', '{bank_a}', 'live-a', 's', 'p2', 'sealed-fixture',
                     'fact', 't0', 't0', 'missing-snap', 'missing-gist', 't0')"
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

    let b = store.integrity_signals(Some(bank_b)).unwrap();
    assert_eq!(b, IntegritySignals::default());

    let all = store.integrity_signals(None).unwrap();
    assert_eq!(all.orphan_assoc_edges, 1);
    assert!(all.orphan_triple_items >= 1);
}

#[test]
fn sqlite_drop_orphan_assoc_edges() {
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
        store
            .integrity_signals(Some(bank))
            .unwrap()
            .orphan_assoc_edges,
        1
    );

    assert_eq!(
        store.drop_orphan_assoc_edges("").unwrap_err().code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        store.drop_orphan_assoc_edges("   ").unwrap_err().code,
        ErrorCode::MissingBank
    );

    assert_eq!(store.drop_orphan_assoc_edges(bank).unwrap(), 1);
    assert_eq!(
        store
            .integrity_signals(Some(bank))
            .unwrap()
            .orphan_assoc_edges,
        0
    );
    assert!(store.get_assoc_edge(bank, "live").unwrap().is_some());
    assert!(store.get_assoc_edge(bank, "orphan").unwrap().is_none());
}
