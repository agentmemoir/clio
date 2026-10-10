// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for extra MemTree coverage and concurrency regression tests.
//!
//! ## Owns
//! - Accessor / filter / error-arm coverage for MemTree types and tools
//! - Split-interval regression (sibling spans widest child)
//! - Concurrent write-during-refresh-wave non-blocking regression
//!
//! ## Does not own
//! - Production MemTree behavior under test
//! - Acceptance traceability scenarios (`memtree_tests`)
//!
//! ## Boundary
//!
//! Changes to edge-case coverage scenarios should normally require changes here.
//! Changes to MemTree behavior should not require changes here.
//!
//! Keep this module focused on MemTree coverage and concurrency regressions.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::{AttachLeaf, BankMemTree, MemForest, MemTreeQueryFilter};
use crate::memtree_maint::MemTreeMaint;
use crate::memtree_tools::{consolidate_tool, maintenance_status, memtree_get, memtree_query};
use clio_types::{EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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

/// Passes the convergence loop will make before it calls a stuck refresh a
/// non-convergence. A bound in passes, not seconds, so a loaded host cannot
/// consume it: see the comment at the loop.
const MAX_CONVERGE_PASSES: u32 = 200;

fn sample_item(id: &str, bank: &str, created: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Gist),
        epistemic_kind: EpistemicKind::Fact,
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
fn coverage_accessors_filters_and_errors() {
    let forest = MemForest::default();
    assert!(forest.tree("none").is_none());
    let mut tree = BankMemTree::new("b", 2);
    assert_eq!(tree.bank_id(), "b");
    assert!(tree.root_id().is_none());
    let (lid, _) = tree.attach_leaf(leaf("a", "b", "1", "g")).unwrap();
    assert!(tree.get(&lid).unwrap().is_leaf());
    assert!(tree.leaf_node_id_for_item("a").is_some());
    let _ = tree.attach_leaf(leaf("b2", "b", "2", "g")).unwrap();
    let _ = tree.attach_leaf(leaf("c", "b", "3", "g")).unwrap();
    for i in 0..12 {
        let id = format!("n{i}");
        let _ = tree.attach_leaf(leaf(&id, "b", &format!("{:02}", 10 + i), &id));
    }
    assert!(tree.root_id().is_some());
    let err = tree.attach_leaf(leaf("a", "b", "99", "dup")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let forced = tree.force_dirty_all_internal();
    assert!(!forced.is_empty(), "{forced:?}");
    let q = tree.query(&MemTreeQueryFilter {
        task_id: Some("nope".into()),
        limit: Some(5),
        ..Default::default()
    });
    assert!(q.is_empty(), "an unknown task matched a node: {q:?}");
}

#[test]
fn coverage_maint_tools_and_force() {
    let leaves = Arc::new(LeafIndex::new());
    leaves.publish(sample_item("i1", "b1", "1", "g"));
    let forest = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    match maintenance_status(&maint, "", None) {
        clio_types::ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::MissingBank),
        clio_types::ToolResult::Ok { .. } => panic!("expected missing bank"),
    }
    match consolidate_tool(
        &maint,
        None,
        None,
        "",
        clio_config::retention::ResolvedRetention::default(),
        false,
        false,
    ) {
        clio_types::ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::MissingBank),
        clio_types::ToolResult::Ok { .. } => panic!("expected missing bank"),
    }
    match memtree_get(&forest, "", "x") {
        clio_types::ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::MissingBank),
        clio_types::ToolResult::Ok { .. } => panic!("expected missing bank"),
    }
    match memtree_query(&forest, "", &MemTreeQueryFilter::default()) {
        clio_types::ToolResult::Err { code, .. } => assert_eq!(code, ErrorCode::MissingBank),
        clio_types::ToolResult::Ok { .. } => panic!("expected missing bank"),
    }
    let st = match maintenance_status(&maint, "ghost", None) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert!(st.leaf_queryable);
    assert!(st.dirty_ancestors.is_empty(), "{:?}", st.dirty_ancestors);
    maint.on_leaf_attached(MaintenanceSignal::leaf_attached("i1", "b1"));
    std::thread::sleep(Duration::from_millis(20));
    let _ = consolidate_tool(
        &maint,
        None,
        None,
        "b1",
        clio_config::retention::ResolvedRetention::default(),
        true,
        true,
    );
    let refreshed = maint.refresh_bank("ghost");
    assert!(refreshed.is_empty(), "refreshed: {refreshed:?}");
    assert!(MemTreeMaint::summarize_children(&[]).contains("aggregate[0]"));
    let dirty = {
        let mut g = forest.lock().unwrap();
        g.tree_mut("err")
            .attach_leaf(leaf("e1", "err", "1", "e"))
            .unwrap()
            .1
    };
    let fail_id = dirty[0].clone();
    maint.fail_refresh_node(&fail_id);
    maint.enqueue_dirty("err", &dirty);
    let _ = maint.refresh_bank("err");
    let st = match maintenance_status(&maint, "err", Some("e1")) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert!(st.last_error.is_some());
    let st2 = match maintenance_status(&maint, "err", None) {
        clio_types::ToolResult::Ok { data, .. } => data,
        clio_types::ToolResult::Err { message, .. } => panic!("{message}"),
    };
    assert!(!st2.dirty_ancestors.is_empty(), "{:?}", st2.dirty_ancestors);
}

#[test]
fn split_sibling_interval_covers_widest_child() {
    // A mid-partition child with a far-reaching interval_end must not be
    // truncated by the sibling split (sibling end = max over children).
    let mut tree = BankMemTree::new("b", 2);
    let mut leaf_ids = Vec::new();
    for (i, (start, end)) in [("10", "10"), ("11", "99"), ("12", "12")]
        .iter()
        .enumerate()
    {
        let (id, _) = tree
            .attach_leaf(AttachLeaf {
                item_id: format!("i{i}"),
                bank_id: "b".into(),
                interval_start: (*start).to_string(),
                interval_end: (*end).to_string(),
                gist: None,
                task_id: None,
            })
            .unwrap();
        leaf_ids.push(id);
    }
    // Root overflowed: children [leaf0, leaf1, leaf2] split into left [leaf0]
    // and a right sibling holding [leaf1 (11→99), leaf2 (12→12)].
    let root = tree.root_id().unwrap().to_owned();
    let root_children = tree.get(&root).unwrap().child_ids.clone();
    let sibling = root_children[1].clone();
    let sib = tree.get(&sibling).unwrap();
    assert_eq!(
        sib.child_ids,
        vec![leaf_ids[1].clone(), leaf_ids[2].clone()]
    );
    assert_eq!(
        sib.interval_end, "99",
        "sibling interval_end must cover the widest child interval"
    );
    assert_eq!(sib.interval_start, "11");
}

#[test]
fn split_sibling_is_enqueued_for_refresh() {
    let mut tree = BankMemTree::new("b", 2);
    tree.attach_leaf(leaf("1", "b", "1", "1")).unwrap();
    tree.attach_leaf(leaf("2", "b", "2", "2")).unwrap();
    let (_, mut dirty) = tree.attach_leaf(leaf("0", "b", "0", "0")).unwrap();
    dirty.sort();
    dirty.dedup();

    let mut all_dirty = tree.dirty_node_ids();
    all_dirty.sort();
    assert_eq!(dirty, all_dirty);
}

#[test]
fn concurrent_writes_during_refresh_wave() {
    // Deep wide tree so the background wave has real work; writes must keep
    // landing through the maintenance hook while the wave is in flight.
    let leaves = Arc::new(LeafIndex::new());
    let forest = Arc::new(Mutex::new(MemForest::with_fanout(2)));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    // Hold the wave open so the late write below provably lands while it is
    // still running. Without this the "a write is not blocked" half of the test
    // races a wave that may already be over on a fast machine.
    maint.set_test_refresh_delay(Duration::from_millis(300));
    {
        let mut g = forest.lock().unwrap();
        let t = g.tree_mut("b1");
        for i in 0..60 {
            let id = format!("w{i}");
            let (_, d) = t
                .attach_leaf(leaf(&id, "b1", &format!("{i:04}"), &id))
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
    // Write arrives while the wave is actively refreshing.
    let maint_bg = Arc::clone(&maint);
    let leaves_bg = Arc::clone(&leaves);
    let start = Instant::now();
    let handle = thread::spawn(move || {
        leaves_bg.publish(sample_item("late", "b1", "9999", "late"));
        maint_bg.on_leaf_attached(MaintenanceSignal::leaf_attached("late", "b1"));
    });
    handle.join().unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "write blocked {elapsed:?} during active refresh wave"
    );
    assert!(leaves.get("b1", "late").is_some());
    // The background wave is still running and holds the dirty set: it requeues
    // whatever it could not refresh, and it competes with this loop for the
    // forest lock and the machine's cores. Converge only once it is done, so the
    // check below is a state check rather than a race against a thread this test
    // cannot join.
    maint.wait_for_quiescence();
    maint.set_test_refresh_delay(Duration::ZERO);
    // Converge: each pass drains the coalesced set, and a pass only leaves a node
    // dirty when refresh genuinely failed, so the dirty set reaches empty.
    //
    // The exit is a state check, not a duration. The bound is a pass count, so
    // the number of iterations available does not shrink when the machine is
    // busy — a wall-clock budget here is consumed by host load rather than by
    // a defect, which is what made this assert "refresh did not converge" under
    // a loaded workspace run. A wall-clock backstop still guards against a
    // genuinely stuck pass, but it is generous enough that ordinary load cannot
    // reach it, and it reports the still-dirty nodes rather than just a time.
    //
    // A pass that makes no progress is a real non-convergence and is reported
    // immediately, so a broken refresh cannot hide behind the remaining budget.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut passes = 0_u32;
    let mut previous_dirty: Option<Vec<String>> = None;
    loop {
        passes += 1;
        let accept2 = match consolidate_tool(
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
        assert_eq!(accept2.status, "completed");
        let dirty_left = forest
            .lock()
            .unwrap()
            .tree("b1")
            .map_or_else(Vec::new, BankMemTree::dirty_node_ids);
        if dirty_left.is_empty() {
            break;
        }
        assert_ne!(
            previous_dirty.as_deref(),
            Some(dirty_left.as_slice()),
            "refresh stopped making progress after {passes} passes; still dirty: {dirty_left:?}"
        );
        assert!(
            Instant::now() < deadline,
            "refresh did not converge in {passes} passes; still dirty: {dirty_left:?}"
        );
        assert!(
            passes < MAX_CONVERGE_PASSES,
            "refresh did not converge within {MAX_CONVERGE_PASSES} passes; still dirty: {dirty_left:?}"
        );
        previous_dirty = Some(dirty_left);
        thread::sleep(Duration::from_millis(10));
    }
}
