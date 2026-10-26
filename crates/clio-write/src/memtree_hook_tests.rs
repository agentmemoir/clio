// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MemTree persistence-hook tests: what the
//! attach hook and the node upsert write, and what they leave alone.
//!
//! ## Owns
//! - The rows an attach, a split, and a refresh wave leave in the store
//! - The paths that write nothing: no store, no published leaf, no such tree
//!
//! ## Does not own
//! - The hook under test
//! - The node/row conversion (`memtree_record_tests`)
//!
//! ## Boundary
//!
//! Changes to which nodes a hook persists should normally require changes here.
//! Changes to the read side of a stored row should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::{AttachLeaf, DEFAULT_FANOUT, MemForest};
use crate::memtree_maint::MemTreeMaint;
use clio_store::{LocalDevKms, MemTreeNodeRecord, MemTreeQuery, SqliteStore, Store};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use std::sync::{Arc, Mutex};

const BANK: &str = "hook-bank";

fn item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "subject-hook".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Gist),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: None,
        gist: Some("gist hook".into()),
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
    let sqlite =
        SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("in-memory store");
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

/// A durable engine over a fresh forest, plus its leaves for publishing.
fn engine(store: &Arc<dyn Store>) -> (Arc<MemTreeMaint>, Arc<LeafIndex>, Arc<Mutex<MemForest>>) {
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let engine =
        MemTreeMaint::with_store(Arc::clone(&forest), Arc::clone(&leaves), Arc::clone(store));
    (engine, leaves, forest)
}

#[test]
fn debug_names_the_durability_of_the_engine() {
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let durable = MemTreeMaint::with_store(Arc::clone(&forest), Arc::clone(&leaves), store());
    let transient = MemTreeMaint::new(Arc::clone(&forest), leaves);
    assert!(
        format!("{durable:?}").contains("durable: true"),
        "{durable:?}"
    );
    assert!(
        format!("{transient:?}").contains("durable: false"),
        "{transient:?}"
    );
}

#[test]
fn a_leaf_signal_persists_the_leaf_and_its_root() {
    let store = store();
    let (engine, leaves, _forest) = engine(&store);
    leaves.publish(item("item-1"));
    engine.on_leaf_attached(MaintenanceSignal::leaf_attached("item-1", BANK));

    let rows = rows(store.as_ref());
    assert_eq!(rows.len(), 2, "leaf + first root: {rows:?}");
    assert!(
        rows.iter().any(|r| r.item_id.as_deref() == Some("item-1")),
        "the leaf row is stored: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.parent_id.is_none() && r.depth == 1),
        "the root row is stored: {rows:?}"
    );
}

#[test]
fn a_signal_for_an_unpublished_item_writes_nothing() {
    let store = store();
    let (engine, _leaves, _forest) = engine(&store);
    engine.on_leaf_attached(MaintenanceSignal::leaf_attached("ghost", BANK));
    assert_eq!(
        rows(store.as_ref()).len(),
        0,
        "an unpublished leaf stores nothing"
    );
}

#[test]
fn a_signal_for_an_already_placed_leaf_still_persists_the_tree() {
    let store = store();
    let (engine, leaves, forest) = engine(&store);
    leaves.publish(item("item-1"));
    // The write path places the leaf itself and then dispatches the signal, so
    // the hook's own attach is the second one and must persist, not bail out.
    forest
        .lock()
        .unwrap()
        .tree_mut(BANK)
        .attach_leaf(AttachLeaf {
            item_id: "item-1".into(),
            bank_id: BANK.into(),
            interval_start: "2026-06-01T00:00:00Z".into(),
            interval_end: "2026-06-01T00:00:00Z".into(),
            gist: Some("gist hook".into()),
            task_id: None,
        })
        .unwrap();

    engine.on_leaf_attached(MaintenanceSignal::leaf_attached("item-1", BANK));

    let rows = rows(store.as_ref());
    assert_eq!(
        rows.len(),
        2,
        "leaf + root despite the earlier attach: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.item_id.as_deref() == Some("item-1")),
        "the placed leaf is stored: {rows:?}"
    );
}

#[test]
fn an_engine_without_a_store_writes_nothing() {
    let store = store();
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let engine = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    leaves.publish(item("item-1"));
    engine.on_leaf_attached(MaintenanceSignal::leaf_attached("item-1", BANK));
    engine.refresh_bank(BANK);
    assert_eq!(
        rows(store.as_ref()).len(),
        0,
        "a store-less engine stores nothing"
    );
}

#[test]
fn a_split_attach_persists_every_node_the_split_moved() {
    let store = store();
    let (engine, leaves, forest) = engine(&store);
    // One more leaf than the fan-out forces a fan-out split, which re-parents
    // children and inserts a sibling: the attach's dirty set alone misses them.
    for n in 0..=DEFAULT_FANOUT {
        let id = format!("item-{n}");
        leaves.publish(item(&id));
        engine.on_leaf_attached(MaintenanceSignal::leaf_attached(&id, BANK));
        let _ = engine.refresh_bank(BANK);
    }
    let tree = forest.lock().unwrap();
    let tree = tree.tree(BANK).expect("bank tree");
    let mirrored = tree
        .query(&crate::memtree::MemTreeQueryFilter::default())
        .iter()
        .map(|n| n.id.clone())
        .filter(|id| {
            store
                .get_memtree_node(BANK, id)
                .expect("stored node")
                .is_some()
        })
        .count();
    assert_eq!(
        mirrored,
        tree.query(&crate::memtree::MemTreeQueryFilter::default())
            .len(),
        "every in-memory node has a stored row after the split"
    );
}

#[test]
fn a_split_attach_stores_the_new_parent_of_every_child_it_moved() {
    let store = store();
    let (engine, leaves, forest) = engine(&store);
    for n in 0..=DEFAULT_FANOUT {
        let id = format!("item-{n}");
        leaves.publish(item(&id));
        engine.on_leaf_attached(MaintenanceSignal::leaf_attached(&id, BANK));
        let _ = engine.refresh_bank(BANK);
    }
    let tree = forest.lock().unwrap();
    let nodes = tree
        .tree(BANK)
        .expect("bank tree")
        .query(&crate::memtree::MemTreeQueryFilter::default());
    let parented: Vec<_> = nodes.iter().filter(|n| n.parent_id.is_some()).collect();
    assert!(
        !parented.is_empty(),
        "the split left children under a parent"
    );
    for node in parented {
        let stored = store
            .get_memtree_node(BANK, &node.id)
            .expect("stored node")
            .unwrap_or_else(|| panic!("no stored row for {}", node.id));
        assert_eq!(
            stored.parent_id, node.parent_id,
            "the stored row of {} names the parent the split moved it to",
            node.id
        );
    }
}

#[test]
fn a_refresh_wave_stores_the_cleared_dirty_flag() {
    let store = store();
    let (engine, leaves, _forest) = engine(&store);
    leaves.publish(item("item-1"));
    engine.on_leaf_attached(MaintenanceSignal::leaf_attached("item-1", BANK));
    assert!(
        rows(store.as_ref()).iter().any(|r| r.dirty),
        "the root starts dirty in the store"
    );
    engine.refresh_bank(BANK);
    let rows = rows(store.as_ref());
    let root = rows
        .iter()
        .find(|r| r.parent_id.is_none())
        .expect("stored root");
    assert!(!root.dirty, "the wave stored the cleared flag: {root:?}");
    assert!(root.summary_gist.is_some(), "{root:?}");
}

#[test]
fn persist_nodes_writes_nothing_for_an_empty_list() {
    let store = store();
    let (engine, _leaves, _forest) = engine(&store);
    engine.persist_nodes(BANK, &[]).expect("no rows to write");
    assert_eq!(rows(store.as_ref()).len(), 0, "no ids, no rows");
}

#[test]
fn persist_nodes_writes_nothing_for_a_bank_the_forest_lacks() {
    let store = store();
    let (engine, _leaves, _forest) = engine(&store);
    engine
        .persist_nodes("bank-without-a-tree", &["node-1".to_owned()])
        .expect("no tree, so no write to fail");
    assert_eq!(rows(store.as_ref()).len(), 0, "no tree, no rows");
}

#[test]
fn a_row_write_the_store_refuses_is_recorded_on_the_engine() {
    let store = store();
    let (engine, leaves, _forest) = engine(&store);
    // The store refuses a memtree row with no bank, so this attach's rows never
    // land. That is a real persist failure against the in-memory store, so the
    // engine must keep it instead of dropping it.
    let mut unbanked = item("item-1");
    unbanked.bank_id = String::new();
    leaves.publish(unbanked);
    engine.on_leaf_attached(MaintenanceSignal::leaf_attached("item-1", ""));

    let (count, last) = engine.persist_failures();
    assert_eq!(count, 1, "the failed upsert is counted once");
    let last = last.expect("the failure message is kept");
    assert!(last.contains("bank"), "{last}");
    assert_eq!(
        rows(store.as_ref()).len(),
        0,
        "nothing landed, which is what the record says"
    );
}
