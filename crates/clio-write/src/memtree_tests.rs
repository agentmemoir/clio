// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MemTree acceptance tests (traceability).
//!
//! ## Owns
//! - Attach / dirty-path / refresh / tool acceptance assertions
//! - Snapshot-safety acceptance and cross-bank isolation
//!
//! ## Does not own
//! - Production MemTree behavior under test
//! - Extra non-acceptance coverage scenarios (`memtree_cov_tests`)
//!
//! ## Boundary
//!
//! Changes to acceptance scenarios should normally require changes here.
//! Changes to MemTree behavior should not require changes here.
//!
//! Keep this module focused on MemTree acceptance coverage.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::{AttachLeaf, BankMemTree, DEFAULT_FANOUT, MemForest, MemTreeQueryFilter};
use crate::memtree_maint::MemTreeMaint;
use crate::memtree_tools::{consolidate_tool, maintenance_status, memtree_get, memtree_query};
use clio_types::{ErrorCode, MemoryItem};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn leaf(item_id: &str, bank: &str, t: &str, gist: &str) -> AttachLeaf {
    AttachLeaf {
        item_id: item_id.into(),
        bank_id: bank.into(),
        interval_start: t.into(),
        interval_end: t.into(),
        gist: Some(gist.into()),
        task_id: None,
    }
}

fn sample_item(id: &str, bank: &str, created: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: clio_types::ItemKind::Episodic,
        category: None,
        episodic_type: Some(clio_types::EpisodicType::Gist),
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: Some("t".into()),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: created.into(),
        valid_until: None,
        tx_from: created.into(),
        tx_until: None,
        created_at: created.into(),
        updated_at: created.into(),
        context: None,
    }
}

#[test]
fn t01_attach_leaf_linked_and_readable() {
    let leaves = Arc::new(LeafIndex::new());
    leaves.publish(sample_item("i1", "b1", "100", "hello"));
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    let (leaf_id, dirty) = forest
        .lock()
        .unwrap()
        .tree_mut("b1")
        .attach_leaf(leaf("i1", "b1", "100", "hello"))
        .unwrap();
    maint.enqueue_dirty("b1", &dirty);
    assert!(!leaf_id.is_empty(), "{leaf_id:?}");
    assert!(!dirty.is_empty(), "{dirty:?}");
    assert!(leaves.get("b1", "i1").is_some());
    let st = match maintenance_status(&maint, "b1", Some("i1")) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert!(st.leaf_queryable);
    // Successful get through the tool (not only the error arm).
    match memtree_get(&forest, "b1", &leaf_id) {
        clio_types::ToolResult::Ok { data, .. } => assert_eq!(data.id, leaf_id),
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    }
}

#[test]
fn t02_dirty_path_only() {
    let mut tree = BankMemTree::new("b1", DEFAULT_FANOUT);
    let (_a, dirty_a) = tree.attach_leaf(leaf("a", "b1", "10", "a")).unwrap();
    // Build a second branch by attaching many leaves to force sibling parents after split.
    for i in 0..DEFAULT_FANOUT {
        let id = format!("x{i}");
        let _ = tree
            .attach_leaf(leaf(&id, "b1", &format!("{:02}", 20 + i), &id))
            .unwrap();
    }
    // Clean all, then attach on one side.
    for id in tree.dirty_node_ids() {
        if let Some(n) = tree.node_mut(&id) {
            n.dirty = false;
        }
    }
    let (_y, dirty_y) = tree.attach_leaf(leaf("y", "b1", "99", "y")).unwrap();
    let dirty_set: std::collections::HashSet<_> = dirty_y.into_iter().collect();
    // Unrelated early ancestors that were cleaned should not all be dirty unless on path.
    for id in dirty_a {
        if !dirty_set.contains(&id)
            && let Some(n) = tree.get(&id)
        {
            assert!(!n.dirty, "unrelated node {id} should stay clean");
        }
    }
}

#[test]
fn t03_coalesce_parent_dirty() {
    let leaves = Arc::new(LeafIndex::new());
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), leaves);
    let d1 = {
        let mut g = forest.lock().unwrap();
        g.tree_mut("b1")
            .attach_leaf(leaf("a", "b1", "1", "a"))
            .unwrap()
            .1
    };
    maint.enqueue_dirty("b1", &d1);
    let d2 = {
        let mut g = forest.lock().unwrap();
        g.tree_mut("b1")
            .attach_leaf(leaf("b", "b1", "2", "b"))
            .unwrap()
            .1
    };
    maint.enqueue_dirty("b1", &d2);
    assert!(maint.coalesce_hits() >= 1);
}

#[test]
fn t04_same_depth_parallel_refresh() {
    let leaves = Arc::new(LeafIndex::new());
    let forest = Arc::new(Mutex::new(MemForest::with_fanout(2)));
    let maint = MemTreeMaint::new(Arc::clone(&forest), leaves);
    {
        let mut g = forest.lock().unwrap();
        let t = g.tree_mut("b1");
        for i in 0..6 {
            let id = format!("i{i}");
            let (_, d) = t
                .attach_leaf(leaf(&id, "b1", &format!("{i}"), &id))
                .unwrap();
            maint.enqueue_dirty("b1", &d);
        }
    }
    let cleared = maint.refresh_bank("b1");
    assert!(!cleared.is_empty(), "{cleared:?}");
    let g = forest.lock().unwrap();
    let t = g.tree("b1").unwrap();
    let dirty_node_ids = t.dirty_node_ids();
    assert!(dirty_node_ids.is_empty(), "{dirty_node_ids:?}");
    let root = t.get(t.root_id().unwrap()).unwrap();
    assert!(!root.dirty);
    assert!(root.summary_gist.is_some());
}

#[test]
fn t05_maintenance_status_while_dirty() {
    let leaves = Arc::new(LeafIndex::new());
    leaves.publish(sample_item("i1", "b1", "1", "g"));
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    let (_, d) = forest
        .lock()
        .unwrap()
        .tree_mut("b1")
        .attach_leaf(leaf("i1", "b1", "1", "g"))
        .unwrap();
    maint.enqueue_dirty("b1", &d);
    let st = match maintenance_status(&maint, "b1", Some("i1")) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert!(st.leaf_queryable);
    assert!(!st.dirty_ancestors.is_empty(), "{:?}", st.dirty_ancestors);
}

#[test]
fn t06_consolidate_nonblocking_with_writes() {
    let leaves = Arc::new(LeafIndex::new());
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    {
        let mut g = forest.lock().unwrap();
        for i in 0..3 {
            let id = format!("i{i}");
            let (_, d) = g
                .tree_mut("b1")
                .attach_leaf(leaf(&id, "b1", &format!("{i}"), &id))
                .unwrap();
            maint.enqueue_dirty("b1", &d);
        }
    }
    let accept = match consolidate_tool(
        &maint,
        None,
        None,
        "b1",
        clio_config::retention::ResolvedRetention::default(),
        false,
        false,
    ) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert_eq!(accept.status, "accepted");
    assert_eq!(accept.hubs, "not_implemented");
    // Concurrent attach while consolidate runs.
    let forest2 = Arc::clone(&forest);
    let handle = thread::spawn(move || {
        forest2
            .lock()
            .unwrap()
            .tree_mut("b1")
            .attach_leaf(leaf("new", "b1", "9", "new"))
            .unwrap()
    });
    let (nid, _) = handle.join().unwrap();
    assert!(!nid.is_empty(), "{nid:?}");
    // `consolidate_tool(wait=false)` claimed a wave slot before spawning the
    // detached refresh, so this waits on that observable slot instead of a
    // fixed guess at how long the wave takes.
    maint.wait_for_quiescence();
}

#[test]
fn t07_refresh_failure_leaves_dirty() {
    let leaves = Arc::new(LeafIndex::new());
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), leaves);
    let dirty = {
        let mut g = forest.lock().unwrap();
        g.tree_mut("b1")
            .attach_leaf(leaf("a", "b1", "1", "a"))
            .unwrap()
            .1
    };
    let fail_id = dirty[0].clone();
    maint.fail_refresh_node(&fail_id);
    maint.enqueue_dirty("b1", &dirty);
    let _ = maint.refresh_bank("b1");
    let g = forest.lock().unwrap();
    let n = g.tree("b1").unwrap().get(&fail_id).unwrap();
    assert!(n.dirty);
    assert!(n.last_error.is_some());
}

#[test]
fn t08_memtree_query_time_range() {
    let forest = Arc::new(Mutex::new(MemForest::new()));
    {
        let mut g = forest.lock().unwrap();
        let t = g.tree_mut("b1");
        t.attach_leaf(leaf("a", "b1", "10", "a")).unwrap();
        t.attach_leaf(leaf("b", "b1", "50", "b")).unwrap();
        t.attach_leaf(leaf("c", "b1", "90", "c")).unwrap();
    }
    let nodes = match memtree_query(
        &forest,
        "b1",
        &MemTreeQueryFilter {
            time_start: Some("40".into()),
            time_end: Some("60".into()),
            depth: Some(0),
            ..Default::default()
        },
    ) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].item_id.as_deref(), Some("b"));
}

#[test]
fn t09_memtree_get_missing() {
    let forest = Arc::new(Mutex::new(MemForest::new()));
    match memtree_get(&forest, "b1", "missing") {
        clio_types::ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::UnknownPath),
        clio_types::ToolResult::Ok { .. } => panic!("expected not found"),
    }
}

#[test]
fn t10_summary_does_not_mutate_leaf_snapshot() {
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let leaves = Arc::new(LeafIndex::new());
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    // Real MemoryItem with structured snapshot bytes (snapshot is authoritative).
    let mut item = sample_item("i1", "b1", "1", "leaf-gist");
    item.snapshot = Some(serde_json::json!({
        "host": "db.internal",
        "port": 5432,
        "captured_at": "2026-09-18T00:00:00Z"
    }));
    let snapshot_before = item.snapshot.clone();
    leaves.publish(item);
    // Attach through the maintenance hook (the real ingest path), then refresh.
    maint.on_leaf_attached(MaintenanceSignal::leaf_attached("i1", "b1"));
    let _ = maint.refresh_bank("b1");
    // Snapshot bytes must be identical after ancestor summary refresh.
    let after = leaves.get("b1", "i1").unwrap();
    assert_eq!(after.snapshot, snapshot_before);
    assert_eq!(after.gist.as_deref(), Some("leaf-gist"));
    // Ancestors were genuinely refreshed and the leaf node is untouched.
    let g = forest.lock().unwrap();
    let t = g.tree("b1").unwrap();
    let leaf_nid = t.leaf_node_id_for_item("i1").unwrap().to_owned();
    assert_eq!(
        t.get(&leaf_nid).unwrap().summary_gist.as_deref(),
        Some("leaf-gist")
    );
    let dirty_left = t.dirty_node_ids();
    assert!(
        dirty_left.is_empty(),
        "ancestors left dirty: {dirty_left:?}"
    );
    for aid in t.ancestors_of(&leaf_nid) {
        let n = t.get(&aid).unwrap();
        assert!(n.summary_gist.is_some(), "ancestor {aid} not summarized");
    }
}

#[test]
fn t11_cross_bank_denied() {
    let forest = Arc::new(Mutex::new(MemForest::new()));
    {
        let mut g = forest.lock().unwrap();
        g.tree_mut("bank-a")
            .attach_leaf(leaf("a", "bank-a", "1", "a"))
            .unwrap();
    }
    let node_id = forest
        .lock()
        .unwrap()
        .tree("bank-a")
        .unwrap()
        .leaf_node_id_for_item("a")
        .unwrap()
        .to_owned();
    match memtree_get(&forest, "bank-b", &node_id) {
        clio_types::ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::UnknownPath),
        clio_types::ToolResult::Ok { .. } => panic!("cross-bank must fail closed"),
    }
    let err = forest
        .lock()
        .unwrap()
        .tree_mut("bank-a")
        .attach_leaf(leaf("x", "bank-b", "1", "x"))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn fanout_default_is_eight() {
    assert_eq!(DEFAULT_FANOUT, 8);
    let t = BankMemTree::new("b", DEFAULT_FANOUT);
    assert_eq!(t.fanout(), 8);
}

#[test]
fn consolidate_wait_reports_structural_duration() {
    let leaves = Arc::new(LeafIndex::new());
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    let dirty = {
        let mut g = forest.lock().unwrap();
        g.tree_mut("b1")
            .attach_leaf(leaf("a", "b1", "1", "a"))
            .unwrap()
            .1
    };
    maint.enqueue_dirty("b1", &dirty);
    maint.set_test_refresh_delay(Duration::from_millis(5));
    leaves.record_time_to_queryable("b1", 7);
    let accept = match consolidate_tool(
        &maint,
        None,
        None,
        "b1",
        clio_config::retention::ResolvedRetention::default(),
        false,
        true,
    ) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert_eq!(accept.status, "completed");
    assert!(!accept.refreshed.is_empty(), "{:?}", accept.refreshed);
    let status = match maintenance_status(&maint, "b1", None) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert!(status.structural_maintenance_ms > 0);
    assert_eq!(status.time_to_queryable_ms, 7);
}

#[test]
fn waves_started_counts_each_refresh_wave() {
    let leaves = Arc::new(LeafIndex::new());
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), leaves);
    let dirty = {
        let mut g = forest.lock().unwrap();
        g.tree_mut("b1")
            .attach_leaf(leaf("a", "b1", "1", "a"))
            .unwrap()
            .1
    };
    maint.enqueue_dirty("b1", &dirty);
    let before = maint.waves_started();
    maint.refresh_bank("b1");
    assert_eq!(
        maint.waves_started(),
        before + 1,
        "one synchronous refresh_bank wave must bump waves_started exactly once"
    );
}
