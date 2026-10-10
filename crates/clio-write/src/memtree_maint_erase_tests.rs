// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Compliance-erasure MemTree propagation coverage (`detach_erased`).
//!
//! Test-only. Never compiled into production builds.

use crate::maintenance::{LeafIndex, MaintenanceHook, MaintenanceSignal};
use crate::memtree::MemForest;
use crate::memtree_maint::MemTreeMaint;
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use std::sync::{Arc, Mutex};

fn item(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subject-erase".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Gist),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.7),
        source_ref: None,
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: "2026-06-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-06-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-06-01T00:00:00Z".into(),
        updated_at: "2026-06-01T00:00:00Z".into(),
        context: None,
    }
}

fn ancestor_summary(forest: &Arc<Mutex<MemForest>>, bank: &str) -> String {
    let guard = forest.lock().expect("forest");
    let tree = guard.tree(bank).expect("tree");
    let root = tree.root_id().expect("root");
    tree.get(root)
        .and_then(|n| n.summary_gist.clone())
        .unwrap_or_default()
}

#[test]
fn detach_erased_removes_leaf_and_regenerates_ancestor() {
    let leaves = Arc::new(LeafIndex::new());
    let forest: Arc<Mutex<MemForest>> = Arc::new(Mutex::new(MemForest::new()));
    let maint = MemTreeMaint::new(Arc::clone(&forest), Arc::clone(&leaves));
    let bank = "erase-bank";

    for (id, gist) in [("e-a", "secret alpha"), ("e-b", "beta text")] {
        leaves.publish(item(id, bank, gist));
        maint.on_leaf_attached(MaintenanceSignal::leaf_attached(id, bank));
    }
    let refreshed = maint.refresh_bank(bank);
    assert!(!refreshed.is_empty(), "{refreshed:?}");
    let before = ancestor_summary(&forest, bank);
    assert!(before.contains("alpha"), "{before}");

    let dirty = maint.detach_erased(bank, &["e-a".to_owned()]);
    assert!(!dirty.is_empty(), "{dirty:?}");
    assert!(leaves.get(bank, "e-a").is_none());
    let after_detach = maint.refresh_bank(bank);
    assert!(!after_detach.is_empty(), "{after_detach:?}");

    let after = ancestor_summary(&forest, bank);
    assert!(!after.contains("alpha"), "{after}");
    assert!(after.contains("beta"), "{after}");

    // A no-op detach on an unknown bank/ids is safe.
    let noop = maint.detach_erased("other-bank", &["x".to_owned()]);
    assert!(noop.is_empty(), "dirty: {noop:?}");
    let noop = maint.detach_erased(bank, &["missing".to_owned()]);
    assert!(noop.is_empty(), "dirty: {noop:?}");
}
