// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use crate::maintenance::{
    BufferMaintenance, LeafIndex, MaintenanceHook, MaintenanceSignal, NoopMaintenance,
};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};

fn item(id: &str, created: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "b".into(),
        subject_id: "s".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: None,
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: created.into(),
        updated_at: created.into(),
        context: None,
    }
}

#[test]
fn leaf_get_list_and_new() {
    let idx = LeafIndex::new();
    idx.publish(item("i2", "t0"));
    idx.publish(item("i1", "t0"));
    idx.publish(item("i0", "t1"));
    assert!(idx.get("b", "i1").is_some());
    assert!(idx.get("b", "missing").is_none());
    let listed = idx.list_leaves("b");
    assert_eq!(listed.len(), 3);
    // same created_at → id tie-break; then later created_at
    assert_eq!(listed[0].id, "i1");
    assert_eq!(listed[1].id, "i2");
    assert_eq!(listed[2].id, "i0");
    let other = idx.list_leaves("other");
    assert!(other.is_empty(), "leaves: {other:?}");
    let _ = LeafIndex::default();
}

#[test]
fn buffer_and_noop_hooks() {
    let hook = BufferMaintenance::default();
    hook.on_leaf_attached(MaintenanceSignal::leaf_attached("i1", "b"));
    assert_eq!(hook.signals().len(), 1);
    let noop = NoopMaintenance;
    noop.on_leaf_attached(MaintenanceSignal::leaf_attached("i2", "b"));
}
