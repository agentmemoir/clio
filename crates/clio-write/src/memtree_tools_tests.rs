// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MemTree read-handler tests: which answer a
//! `memtree_query` / `memtree_get` call returns, and how it reports nothing.
//!
//! ## Owns
//! - Store-first reads, the unflushed-forest fallback, and cross-bank refusal
//! - The not-found shape of an empty query and of an unknown node
//!
//! ## Does not own
//! - The handlers under test
//! - Attach / refresh behaviour (`memtree_tests`, `memtree_cov_tests`)
//!
//! ## Boundary
//!
//! Changes to how a read is answered should normally require changes here.
//! Changes to persistence timing should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::{AttachLeaf, MemForest, MemTreeNode, MemTreeQueryFilter};
use crate::memtree_maint::MemTreeMaint;
use crate::memtree_tools::{memtree_get, memtree_query};
use clio_store::{LocalDevKms, SqliteStore, Store};
use clio_types::{EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, ToolResult};
use std::sync::{Arc, Mutex};

const BANK: &str = "bank-durable";

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
    let sqlite =
        SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("in-memory store");
    Arc::new(sqlite)
}

fn leaf(item_id: &str, bank: &str) -> AttachLeaf {
    AttachLeaf {
        item_id: item_id.into(),
        bank_id: bank.into(),
        interval_start: "2026-06-01T00:00:00Z".into(),
        interval_end: "2026-06-01T00:00:00Z".into(),
        gist: Some("gist one".into()),
        task_id: None,
    }
}

/// Publish one item, attach it through the real hook, and return its forest.
fn attached_durably(store: &Arc<dyn Store>) -> Arc<Mutex<MemForest>> {
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let engine =
        MemTreeMaint::with_store(Arc::clone(&forest), Arc::clone(&leaves), Arc::clone(store));
    leaves.publish(item("item-1"));
    engine.on_leaf_attached(MaintenanceSignal::leaf_attached("item-1", BANK));
    forest
}

fn data(result: ToolResult<Vec<MemTreeNode>>) -> Vec<MemTreeNode> {
    match result {
        ToolResult::Ok { data, .. } => data,
        ToolResult::Err { message, .. } => panic!("{message}"),
    }
}

#[test]
fn query_answers_from_the_store_a_later_process_wrote() {
    let store = store();
    let _writer = attached_durably(&store);
    // A fresh process holds no tree for the bank: only the store can answer.
    let reader: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let nodes = data(memtree_query(
        store.as_ref(),
        &reader,
        BANK,
        &MemTreeQueryFilter::default(),
    ));
    assert_eq!(nodes.len(), 2, "leaf + root from the store: {nodes:?}");
    assert!(
        nodes.iter().all(|n| n.bank_id == BANK),
        "every row is the bank's: {nodes:?}"
    );
}

#[test]
fn query_prefers_stored_rows_over_the_forest_this_process_holds() {
    let store = store();
    let forest = attached_durably(&store);
    // A node this process never flushed: a forest row the store cannot hold.
    forest
        .lock()
        .unwrap()
        .tree_mut(BANK)
        .attach_leaf(leaf("unflushed", BANK))
        .unwrap();
    let nodes = data(memtree_query(
        store.as_ref(),
        &forest,
        BANK,
        &MemTreeQueryFilter::default(),
    ));
    assert_eq!(nodes.len(), 2, "the stored rows only: {nodes:?}");
    assert!(
        nodes
            .iter()
            .all(|n| n.item_id.as_deref() != Some("unflushed")),
        "an unflushed forest node must not displace the stored rows: {nodes:?}"
    );
}

#[test]
fn query_falls_back_to_the_forest_for_an_unflushed_attach() {
    let store = store();
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    forest
        .lock()
        .unwrap()
        .tree_mut(BANK)
        .attach_leaf(leaf("item-1", BANK))
        .unwrap();
    let nodes = data(memtree_query(
        store.as_ref(),
        &forest,
        BANK,
        &MemTreeQueryFilter::default(),
    ));
    assert_eq!(nodes.len(), 2, "the forest's own attach: {nodes:?}");
}

#[test]
fn query_of_an_empty_bank_is_not_found() {
    let store = store();
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    match memtree_query(
        store.as_ref(),
        &forest,
        "bank-empty",
        &MemTreeQueryFilter::default(),
    ) {
        ToolResult::Err { code, message, .. } => {
            assert_eq!(code, ErrorCode::NotFound);
            assert!(message.contains("bank-empty"), "{message}");
            assert!(!message.contains("time range"), "{message}");
        }
        ToolResult::Ok { .. } => panic!("an empty bank is not an empty success"),
    }
}

#[test]
fn query_of_an_empty_time_range_names_the_range() {
    let store = store();
    let _writer = attached_durably(&store);
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let filter = MemTreeQueryFilter {
        time_start: Some("2030-01-01".into()),
        time_end: Some("2031-01-01".into()),
        ..Default::default()
    };
    match memtree_query(store.as_ref(), &forest, BANK, &filter) {
        ToolResult::Err { code, message, .. } => {
            assert_eq!(code, ErrorCode::NotFound);
            assert!(message.contains(BANK), "{message}");
            assert!(
                message.contains("2030-01-01") && message.contains("2031-01-01"),
                "{message}"
            );
        }
        ToolResult::Ok { data, .. } => panic!("expected not found, got {} nodes", data.len()),
    }
}

#[test]
fn get_reads_a_stored_node_for_a_bank_the_forest_lacks() {
    let store = store();
    let _writer = attached_durably(&store);
    let leaf_id = store
        .query_memtree_nodes(&clio_store::MemTreeQuery {
            bank_id: BANK.into(),
            ..Default::default()
        })
        .expect("rows")
        .into_iter()
        .find(|r| r.item_id.as_deref() == Some("item-1"))
        .expect("persisted leaf")
        .id;
    let reader: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    match memtree_get(store.as_ref(), &reader, BANK, &leaf_id) {
        ToolResult::Ok { data, .. } => {
            assert_eq!(data.id, leaf_id);
            assert_eq!(data.depth, 0);
        }
        ToolResult::Err { message, .. } => panic!("{message}"),
    }
}

#[test]
fn get_falls_back_to_an_unflushed_forest_node() {
    let store = store();
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let leaf_id = forest
        .lock()
        .unwrap()
        .tree_mut(BANK)
        .attach_leaf(leaf("item-1", BANK))
        .unwrap()
        .0;
    match memtree_get(store.as_ref(), &forest, BANK, &leaf_id) {
        ToolResult::Ok { data, .. } => assert_eq!(data.id, leaf_id),
        ToolResult::Err { message, .. } => panic!("{message}"),
    }
}

#[test]
fn get_of_an_unknown_node_is_not_found() {
    let store = store();
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    match memtree_get(store.as_ref(), &forest, BANK, "missing") {
        ToolResult::Err { code, message, .. } => {
            assert_eq!(code, ErrorCode::NotFound);
            assert!(message.contains("missing"), "{message}");
            assert!(message.contains(BANK), "{message}");
        }
        ToolResult::Ok { .. } => panic!("expected not found"),
    }
}

#[test]
fn get_refuses_a_node_of_another_bank() {
    let store = store();
    let forest = attached_durably(&store);
    let leaf_id = store
        .query_memtree_nodes(&clio_store::MemTreeQuery {
            bank_id: BANK.into(),
            ..Default::default()
        })
        .expect("rows")
        .into_iter()
        .find(|r| r.item_id.as_deref() == Some("item-1"))
        .expect("persisted leaf")
        .id;
    match memtree_get(store.as_ref(), &forest, "bank-other", &leaf_id) {
        ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::NotFound),
        ToolResult::Ok { .. } => panic!("cross-bank must fail closed"),
    }
}

#[test]
fn both_reads_require_a_bank() {
    let store = store();
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    match memtree_get(store.as_ref(), &forest, "", "x") {
        ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::MissingBank),
        ToolResult::Ok { .. } => panic!("expected missing bank"),
    }
    match memtree_query(store.as_ref(), &forest, "", &MemTreeQueryFilter::default()) {
        ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::MissingBank),
        ToolResult::Ok { .. } => panic!("expected missing bank"),
    }
}
