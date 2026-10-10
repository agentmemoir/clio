// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the Postgres `OpsStore` operator-health
//! contract on the shared Compose database: reachability, recorded schema
//! version, bank inventory, content-free integrity counts, and orphan edge
//! cleanup.
//!
//! ## Owns
//! - Postgres parity for the `OpsStore` contract, including planted-orphan
//!   counts scoped to a unique bank prefix per test
//!
//! ## Does not own
//! - The contract definition (`ops_store`) or the SQL under test (`postgres_ops`)
//! - SQLite behavior for the same contract (`ops_store_tests`)
//!
//! ## Boundary
//!
//! Tests here break when Postgres operator-health SQL changes; they never change it.
//!
//! Keep this module focused on Postgres operator-health tests.

use crate::db_url::postgres_url_from_env;
use crate::model::{AssocEdgeRecord, AssocKind};
use crate::ops_store::{IntegritySignals, OpsStore};
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::{DekProvider, LocalDevKms};
use clio_types::{EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn unique(prefix: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{}-{n}", std::process::id())
}

fn pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    PostgresStore::open(&url, kms).expect("postgres required")
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

/// Remove every row this suite created for `bank` (children before items).
fn cleanup(pg: &PostgresStore, bank: &str) {
    let _ = pg.test_execute(&format!(
        "DELETE FROM triples WHERE bank_id = '{bank}';
         DELETE FROM assoc_edges WHERE bank_id = '{bank}';
         DELETE FROM index_pending WHERE bank_id = '{bank}';
         DELETE FROM memtree_nodes WHERE bank_id = '{bank}';
         DELETE FROM items WHERE bank_id = '{bank}';"
    ));
}

#[test]
fn pg_ping_and_schema_version() {
    let _guard = crate::lock_pg_test();
    let pg = pg();
    assert!(pg.ping().is_ok());
    assert_eq!(pg.schema_version().unwrap().as_deref(), Some("1"));
    pg.test_execute("DELETE FROM schema_settings WHERE key = 'schema_version'")
        .unwrap();
    assert!(pg.schema_version().unwrap().is_none());
    // A hand-written stale row is echoed back verbatim; the bump to the current
    // version happens on open, not on read.
    pg.test_execute(
        "INSERT INTO schema_settings (key, value, created_at, updated_at)
         VALUES ('schema_version', '11', 't0', 't0')
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .unwrap();
    assert_eq!(pg.schema_version().unwrap().as_deref(), Some("11"));
}

#[test]
fn pg_bank_ids_scoped_and_sorted() {
    let _guard = crate::lock_pg_test();
    let pg = pg();
    let bank_a = unique("ops-pg-a");
    let bank_b = unique("ops-pg-b");
    pg.create_memory_item(&item(&unique("i-b"), &bank_b), "actor")
        .unwrap();
    pg.create_memory_item(&item(&unique("i-a"), &bank_a), "actor")
        .unwrap();

    let ids = pg.bank_ids().unwrap();
    assert!(ids.contains(&bank_a));
    assert!(ids.contains(&bank_b));
    // `bank_ids()` returns database-collation order, and the shared database
    // also holds banks from other tests and earlier runs. Assert ordering only
    // for the banks this test created; their names differ in a plain ASCII
    // letter, which every collation sorts the same way.
    let mine: Vec<&String> = ids
        .iter()
        .filter(|id| **id == bank_a || **id == bank_b)
        .collect();
    assert!(mine.windows(2).all(|w| w[0] <= w[1]));

    cleanup(&pg, &bank_a);
    cleanup(&pg, &bank_b);
}

#[test]
fn pg_counts_memories_and_distinct_subjects() {
    let _guard = crate::lock_pg_test();
    let pg = pg();
    let bank = unique("ops-pg-counts");
    let before = pg.counts().unwrap();

    for (id, subject) in [
        ("a", "pg-subject-a"),
        ("b", "pg-subject-a"),
        ("c", "pg-subject-b"),
    ] {
        let mut row = item(&unique(&format!("counts-{id}")), &bank);
        row.subject_id = subject.to_owned();
        pg.create_memory_item(&row, "actor").unwrap();
    }

    let after = pg.counts().unwrap();
    assert_eq!(after.memories, before.memories + 3);
    assert_eq!(after.subjects, before.subjects + 2);

    cleanup(&pg, &bank);
    assert_eq!(pg.counts().unwrap(), before);
}

#[test]
fn pg_integrity_signals_and_orphans() {
    let _guard = crate::lock_pg_test();
    let pg = pg();
    let bank_a = unique("ops-pg-int");
    let bank_b = unique("ops-pg-int-b");
    assert_eq!(
        pg.integrity_signals(Some(&bank_a)).unwrap(),
        IntegritySignals::default()
    );

    let live = unique("live-a");
    pg.create_memory_item(&item(&live, &bank_a), "actor")
        .unwrap();
    pg.put_assoc_edge(&edge(&unique("orphan-edge"), &bank_a, "ghost", &live))
        .unwrap();
    pg.test_execute(&format!(
        "SET session_replication_role = replica;
         INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext,
                              epistemic_kind, valid_from, tx_from, created_at)
         VALUES ('{}', '{bank_a}', 'missing-item', 's', 'p', 'sealed-fixture',
                 'fact', 't0', 't0', 't0');
         SET session_replication_role = origin;",
        unique("orphan-triple")
    ))
    .unwrap();
    pg.test_execute(&format!(
        "INSERT INTO triples (id, bank_id, item_id, subject, predicate, object_ciphertext,
                              epistemic_kind, valid_from, tx_from, snapshot_ref,
                              gist_ref, created_at)
         VALUES ('{}', '{bank_a}', '{live}', 's', 'p2', 'sealed-fixture',
                 'fact', 't0', 't0', 'missing-snap', 'missing-gist', 't0')",
        unique("dangling-triple")
    ))
    .unwrap();
    pg.test_execute(&format!(
        "INSERT INTO memtree_nodes (id, bank_id, item_id, depth, interval_start,
                                    interval_end, child_ids_json, dirty, generation,
                                    refreshing, created_at, updated_at)
         VALUES ('{}', '{bank_a}', 'missing-item', 0, 't0', 't0', '[]',
                 0, 0, 0, 't0', 't0')",
        unique("orphan-leaf")
    ))
    .unwrap();

    let a = pg.integrity_signals(Some(&bank_a)).unwrap();
    assert_eq!(a.orphan_assoc_edges, 1);
    assert_eq!(a.orphan_triple_items, 1);
    assert_eq!(a.dangling_snapshot_refs, 1);
    assert_eq!(a.dangling_gist_refs, 1);
    assert_eq!(a.orphan_memtree_leaf_items, 1);

    let b = pg.integrity_signals(Some(&bank_b)).unwrap();
    assert_eq!(b, IntegritySignals::default());

    cleanup(&pg, &bank_a);
}

#[test]
fn pg_drop_orphan_assoc_edges() {
    let _guard = crate::lock_pg_test();
    let pg = pg();
    let bank = unique("ops-pg-drop");
    let live_1 = unique("live-1");
    let live_2 = unique("live-2");
    pg.create_memory_item(&item(&live_1, &bank), "actor")
        .unwrap();
    pg.create_memory_item(&item(&live_2, &bank), "actor")
        .unwrap();
    let orphan_id = unique("orphan");
    let live_id = unique("live");
    pg.put_assoc_edge(&edge(&orphan_id, &bank, "ghost", &live_1))
        .unwrap();
    pg.put_assoc_edge(&edge(&live_id, &bank, &live_1, &live_2))
        .unwrap();
    assert_eq!(
        pg.integrity_signals(Some(&bank))
            .unwrap()
            .orphan_assoc_edges,
        1
    );

    assert_eq!(
        pg.drop_orphan_assoc_edges("").unwrap_err().code,
        ErrorCode::MissingBank
    );
    assert_eq!(pg.drop_orphan_assoc_edges(&bank).unwrap(), 1);
    assert_eq!(
        pg.integrity_signals(Some(&bank))
            .unwrap()
            .orphan_assoc_edges,
        0
    );
    assert!(pg.get_assoc_edge(&bank, &live_id).unwrap().is_some());
    assert!(pg.get_assoc_edge(&bank, &orphan_id).unwrap().is_none());

    cleanup(&pg, &bank);
}
