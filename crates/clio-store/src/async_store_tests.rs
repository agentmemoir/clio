// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Tests for [`crate::async_store`].

use crate::async_store::{AsyncStore, BlockingAsyncStore};
use crate::dek::LocalDevKms;
use crate::model::{AssocEdgeRecord, AssocKind, EMBEDDING_DIMS, VectorSlot};
use crate::sqlite::SqliteStore;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use std::sync::Arc;

fn typed_item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "b".into(),
        subject_id: "s".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.0),
        source_ref: Some("turn_9".into()),
        context: None,
        snapshot: Some(serde_json::json!({"hi": true})),
        gist: Some("hi".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn blocking_adapter_round_trip() {
    let store = Arc::new(SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap());
    let async_store = BlockingAsyncStore::new(store);
    async_store
        .create_memory_item(typed_item("async-1"), "actor".into())
        .await
        .unwrap();
    let got = async_store
        .get_memory_item("b".into(), "async-1".into())
        .await
        .unwrap()
        .expect("present");
    assert_eq!(got.gist.as_deref(), Some("hi"));
    let mut emb = vec![0.0_f32; EMBEDDING_DIMS];
    emb[0] = 1.0;
    async_store
        .upsert_vector(VectorSlot {
            item_id: "async-1".into(),
            bank_id: "b".into(),
            model_id: crate::DEFAULT_EMBEDDING_MODEL.into(),
            embedding: emb.clone(),
        })
        .await
        .unwrap();
    let hits = async_store.knn("b".into(), emb, 1).await.unwrap();
    assert_eq!(hits[0].item_id, "async-1");
    assert!(
        async_store
            .delete_vector("b".into(), "async-1".into())
            .await
            .unwrap()
    );
    let edge = AssocEdgeRecord {
        id: "e1".into(),
        bank_id: "b".into(),
        source_id: "a".into(),
        target_id: "c".into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("r".into()),
        weight: 1.0,
        last_updated_at: "t".into(),
    };
    async_store.put_assoc_edge(edge).await.unwrap();
    let got = async_store
        .get_assoc_edge("b".into(), "e1".into())
        .await
        .unwrap()
        .expect("edge");
    assert_eq!(got.id, "e1");
    assert!(
        async_store
            .delete_assoc_edge("b".into(), "e1".into())
            .await
            .unwrap()
    );
    let reinforced = async_store
        .reinforce_assoc_edge("b".into(), "a".into(), "c".into(), 0, 0.15)
        .await
        .unwrap();
    assert!((reinforced.weight - 0.15).abs() < 1e-12);
    assert_eq!(
        async_store
            .list_assoc_edges("b".into(), Some("a".into()))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        async_store
            .list_assoc_edges("b".into(), None)
            .await
            .unwrap()
            .len(),
        1
    );
    async_store
        .record_assoc_audit(
            "b".into(),
            "assoc_prune".into(),
            Some("e".into()),
            "agent".into(),
            serde_json::json!({"effective_weight": 0.01}),
        )
        .await
        .unwrap();
    assert!(
        async_store
            .delete_item("b".into(), "async-1".into())
            .await
            .unwrap()
    );
    async_store
        .upsert_subject_key_ref("s".into(), "local:s:v1".into(), "active".into())
        .await
        .unwrap();
    assert!(
        async_store
            .inspect_content_ciphertext("b".into(), "async-1".into())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn blocking_adapter_memtree_round_trip() {
    let store = Arc::new(SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap());
    let async_store = BlockingAsyncStore::new(store);
    let node = crate::memtree_model::MemTreeNodeRecord {
        id: "mt1".into(),
        bank_id: "b".into(),
        parent_id: None,
        item_id: Some("async-1".into()),
        depth: 0,
        interval_start: "100".into(),
        interval_end: "100".into(),
        child_ids: vec![],
        summary_gist: Some("g".into()),
        dirty: false,
        generation: 0,
        refreshing: false,
        task_id: None,
        last_error: None,
        created_at: "100".into(),
        updated_at: "100".into(),
    };
    async_store.upsert_memtree_node(node).await.unwrap();
    let got = async_store
        .get_memtree_node("b".into(), "mt1".into())
        .await
        .unwrap()
        .expect("memtree node");
    assert_eq!(got.id, "mt1");
    let listed = async_store
        .query_memtree_nodes(crate::memtree_model::MemTreeQuery {
            bank_id: "b".into(),
            time_start: None,
            time_end: None,
            depth: Some(0),
            task_id: None,
            limit: Some(10),
        })
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
}
