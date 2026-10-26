// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MemTree durable-write tests: which rows a
//! leaf attach, a refresh wave, and a re-persist leave in the store.
//!
//! ## Owns
//! - The leaf and root rows an attach signal creates, and the gist row a refresh
//!   wave rewrites while clearing the dirty flag
//! - The paths that write nothing: an engine with no store, a re-persisted node
//!   keeping its original creation time
//!
//! ## Does not own
//! - The maintenance hook and record conversion under test
//! - The store's own SQLite behaviour
//!
//! ## Boundary
//!
//! Changes to what a durable MemTree write stores should normally require changes here.
//! Changes to the hook, the record shape, or the store itself should not require changes here.
//!
//! Keep this module focused on the rows a write is expected to leave behind.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::{MemForest, MemTreeNode};
use crate::memtree_maint::MemTreeMaint;
use crate::memtree_record::record_from_node;
use clio_store::{LocalDevKms, MemTreeNodeRecord, MemTreeQuery, SqliteStore, Store};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use std::sync::{Arc, Mutex};

const BANK: &str = "durable-bank";

fn item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "subject-durable".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Gist),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: None,
        gist: Some("gist one".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

fn store() -> Arc<dyn Store> {
    let sqlite = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("in-memory store");
    Arc::new(sqlite)
}

fn rows(store: &dyn Store) -> Vec<MemTreeNodeRecord> {
    store
        .query_memtree_nodes(&MemTreeQuery {
            bank_id: BANK.into(),
            ..MemTreeQuery::default()
        })
        .expect("memtree query")
}

/// Attach one leaf through the hook and return the engine plus its forest.
fn attached(store: Option<Arc<dyn Store>>) -> (Arc<MemTreeMaint>, Arc<Mutex<MemForest>>) {
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let maint = match store {
        Some(store) => MemTreeMaint::with_store(Arc::clone(&forest), Arc::clone(&leaves), store),
        None => MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves)),
    };
    leaves.publish(item("item-1"));
    maint.on_leaf_attached(MaintenanceSignal::leaf_attached("item-1", BANK));
    (maint, forest)
}

#[test]
fn a_leaf_signal_persists_the_leaf_and_its_root() {
    let store = store();
    let _engine = attached(Some(Arc::clone(&store)));

    let rows = rows(store.as_ref());
    assert_eq!(rows.len(), 2, "leaf + its first root: {rows:?}");
    let leaf = rows
        .iter()
        .find(|r| r.item_id.as_deref() == Some("item-1"))
        .expect("persisted leaf row");
    let root = rows
        .iter()
        .find(|r| r.parent_id.is_none())
        .expect("persisted root row");
    assert_eq!(leaf.parent_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(leaf.depth, 0);
    assert_eq!(root.depth, 1);
    assert_eq!(leaf.interval_start, "2026-06-01T00:00:00Z");
    assert_eq!(leaf.interval_end, "2026-06-01T00:00:00Z");
    assert_eq!(root.child_ids, vec![leaf.id.clone()]);
    assert_eq!(root.interval_start, leaf.interval_start);
}

#[test]
fn a_refresh_wave_stores_the_new_gist_and_clears_dirty() {
    let store = store();
    let (engine, _forest) = attached(Some(Arc::clone(&store)));
    assert!(
        rows(store.as_ref()).iter().any(|r| r.dirty),
        "the root starts dirty in the store"
    );

    engine.refresh_bank(BANK);

    let rows = rows(store.as_ref());
    let root = rows
        .iter()
        .find(|r| r.parent_id.is_none())
        .expect("persisted root row");
    assert!(!root.dirty, "refresh cleared the stored dirty flag");
    assert!(
        root.summary_gist.is_some(),
        "refresh stored the recomputed gist: {root:?}"
    );
    assert_eq!(root.last_error, None);
}

#[test]
fn an_engine_without_a_store_writes_nothing_and_does_not_panic() {
    let probe = store();
    let (_engine, forest) = attached(None);

    assert!(
        rows(probe.as_ref()).is_empty(),
        "a store-less engine persists no rows"
    );
    let guard = forest.lock().expect("forest");
    assert!(
        guard.tree(BANK).and_then(|t| t.root_id()).is_some(),
        "the in-memory tree still placed the leaf"
    );
}

#[test]
fn re_persisting_a_node_keeps_the_first_created_at() {
    let store = store();
    let node = MemTreeNode {
        id: "bank-a:leaf:1".into(),
        bank_id: "bank-a".into(),
        parent_id: None,
        item_id: Some("item-1".into()),
        depth: 0,
        interval_start: "2026-06-01T00:00:00Z".into(),
        interval_end: "2026-06-01T00:00:00Z".into(),
        child_ids: vec![],
        summary_gist: Some("first".into()),
        dirty: false,
        generation: 0,
        refreshing: false,
        task_id: None,
        last_error: None,
    };
    store
        .upsert_memtree_node(&record_from_node(&node, "2026-06-01T00:00:00Z"))
        .expect("first write");
    store
        .upsert_memtree_node(&record_from_node(&node, "2026-06-02T00:00:00Z"))
        .expect("second write");

    let stored = store
        .get_memtree_node("bank-a", "bank-a:leaf:1")
        .expect("get")
        .expect("row");
    assert_eq!(stored.created_at, "2026-06-01T00:00:00Z");
    assert_eq!(stored.updated_at, "2026-06-02T00:00:00Z");
}
