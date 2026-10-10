// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the non-blocking maintenance reads: the
//! dirty-ancestor count must agree with the blocking status read, and a held
//! forest lock must surface as contention rather than as a count.
//!
//! ## Owns
//! - Test-only coverage of `MemTreeMaint::try_dirty_ancestor_count`
//!
//! ## Does not own
//! - The reads themselves (`memtree_maint_reads`)
//! - Status response assembly (`memtree_tools`)
//!
//! ## Boundary
//!
//! Changes to what a non-blocking read answers should normally require changes
//! here. Changes to how a wave is run should not require changes here.
//!
//! Keep this module focused on the non-blocking read contract.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::MemForest;
use crate::memtree_maint::MemTreeMaint;
use crate::memtree_tools::maintenance_status;
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem, ToolResult};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

const BANK: &str = "reads-bank";

fn maint() -> (Arc<MemTreeMaint>, Arc<Mutex<MemForest>>) {
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    (MemTreeMaint::new(Arc::clone(&forest), leaves), forest)
}

fn item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "subject-reads".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Gist),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: None,
        gist: Some("reads fixture summary".into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

/// Attach one leaf, which leaves its ancestors dirty.
fn dirty_bank(maint: &MemTreeMaint) {
    maint.leaves.publish(item("r-1"));
    maint.on_leaf_attached(MaintenanceSignal::leaf_attached("r-1", BANK));
}

/// The count the blocking status read reports for a whole bank.
fn blocking_dirty_count(maint: &MemTreeMaint, bank: &str) -> usize {
    match maintenance_status(maint, bank, None) {
        ToolResult::Ok { data, .. } => data.dirty_ancestors.len(),
        ToolResult::Err { .. } => panic!("blocking status read failed for {bank}"),
    }
}

#[test]
fn the_non_blocking_count_matches_the_blocking_status_read() {
    let (maint, _forest) = maint();
    assert_eq!(
        maint.try_dirty_ancestor_count(BANK),
        Some(blocking_dirty_count(&maint, BANK)),
        "an unknown bank reads as zero on both paths"
    );

    dirty_bank(&maint);
    let dirty = blocking_dirty_count(&maint, BANK);
    assert!(dirty > 0, "the attached leaf left no dirty ancestor");
    assert_eq!(
        maint.try_dirty_ancestor_count(BANK),
        Some(dirty),
        "the non-blocking read must agree with the blocking read while dirty"
    );

    maint.refresh_bank(BANK);
    assert_eq!(
        maint.try_dirty_ancestor_count(BANK),
        Some(blocking_dirty_count(&maint, BANK)),
        "the non-blocking read must agree with the blocking read once clean"
    );
    assert_eq!(maint.try_dirty_ancestor_count(BANK), Some(0));
}

#[test]
fn a_held_forest_lock_reads_as_contention_not_as_zero() {
    let (maint, forest) = maint();
    dirty_bank(&maint);
    let dirty = maint.try_dirty_ancestor_count(BANK);
    assert_eq!(dirty, Some(blocking_dirty_count(&maint, BANK)));

    // A holder thread keeps the lock until this test releases it, so the
    // contended read below is a real lock failure rather than a race.
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = thread::spawn(move || {
        let _guard = forest.lock().expect("forest");
        locked_tx.send(()).expect("signal the lock is held");
        release_rx.recv().expect("wait for release");
    });
    locked_rx.recv().expect("holder took the forest lock");

    assert_eq!(
        maint.try_dirty_ancestor_count(BANK),
        None,
        "a held lock must report contention, never a zero count"
    );

    release_tx.send(()).expect("release the holder");
    holder.join().expect("join the holder");
    assert_eq!(
        maint.try_dirty_ancestor_count(BANK),
        dirty,
        "the same bank reads the same count once the lock is free"
    );
}
