// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Async surface over the sync [`Store`] contract via `spawn_blocking`.
//!
//! Drivers stay sync (rusqlite / `postgres` crate). Callers that need async
//! use this adapter. Native pools (`deadpool` / `sqlx` / `tokio-postgres`) are
//! the scale upgrade when multiplexed concurrent DB I/O is required.

use crate::model::{AssocEdgeRecord, KnnHit, VectorSlot};
use crate::store::Store;
use async_trait::async_trait;
use clio_types::{AmError, MemoryItem};
use serde_json::Value;
use std::sync::Arc;

/// Async mirror of [`Store`] for Tokio callers.
#[async_trait]
pub trait AsyncStore: Send + Sync {
    /// Delete one item row and its derived rows, scoped by bank.
    async fn delete_item(&self, bank_id: String, item_id: String) -> Result<bool, AmError>;
    /// Ungated typed memory-item create.
    async fn create_memory_item(&self, item: MemoryItem, actor: String) -> Result<(), AmError>;
    /// Ungated typed memory-item update.
    async fn update_memory_item(&self, item: MemoryItem, actor: String) -> Result<(), AmError>;
    /// Read a typed memory item.
    async fn get_memory_item(
        &self,
        bank_id: String,
        item_id: String,
    ) -> Result<Option<MemoryItem>, AmError>;
    /// Authoritative snapshot half only.
    async fn get_snapshot(
        &self,
        bank_id: String,
        item_id: String,
    ) -> Result<Option<Value>, AmError>;
    /// Non-authoritative gist half only.
    async fn get_gist(&self, bank_id: String, item_id: String) -> Result<Option<String>, AmError>;
    /// Insert or replace an association edge.
    async fn put_assoc_edge(&self, edge: AssocEdgeRecord) -> Result<(), AmError>;
    /// Read an association edge.
    async fn get_assoc_edge(
        &self,
        bank_id: String,
        edge_id: String,
    ) -> Result<Option<AssocEdgeRecord>, AmError>;
    /// Delete an association edge.
    async fn delete_assoc_edge(&self, bank_id: String, edge_id: String) -> Result<bool, AmError>;
    /// List association edges (whole bank when `item_id` is `None`).
    async fn list_assoc_edges(
        &self,
        bank_id: String,
        item_id: Option<String>,
    ) -> Result<Vec<AssocEdgeRecord>, AmError>;
    /// Atomically apply one saturating co-activation growth step.
    async fn reinforce_assoc_edge(
        &self,
        bank_id: String,
        a: String,
        b: String,
        now_millis: i64,
        eta: f64,
    ) -> Result<AssocEdgeRecord, AmError>;
    /// Append a PII-safe association audit event.
    async fn record_assoc_audit(
        &self,
        bank_id: String,
        operation: String,
        item_id: Option<String>,
        actor: String,
        detail: Value,
    ) -> Result<(), AmError>;
    /// Upsert a dense vector slot.
    async fn upsert_vector(&self, slot: VectorSlot) -> Result<(), AmError>;
    /// Delete a dense vector slot.
    async fn delete_vector(&self, bank_id: String, item_id: String) -> Result<bool, AmError>;
    /// Exact/brute-force KNN (cosine).
    async fn knn(&self, bank_id: String, query: Vec<f32>, k: usize)
    -> Result<Vec<KnnHit>, AmError>;
    /// Persist / refresh `subject_keys.dek_ref`.
    async fn upsert_subject_key_ref(
        &self,
        subject_id: String,
        dek_ref: String,
        status: String,
    ) -> Result<(), AmError>;
    /// Read raw `content_ciphertext` for inspection tests.
    async fn inspect_content_ciphertext(
        &self,
        bank_id: String,
        item_id: String,
    ) -> Result<Option<String>, AmError>;
    /// Upsert a MemTree node.
    async fn upsert_memtree_node(
        &self,
        node: crate::memtree_model::MemTreeNodeRecord,
    ) -> Result<(), AmError>;
    /// Get a MemTree node by bank + id.
    async fn get_memtree_node(
        &self,
        bank_id: String,
        node_id: String,
    ) -> Result<Option<crate::memtree_model::MemTreeNodeRecord>, AmError>;
    /// Query MemTree nodes.
    async fn query_memtree_nodes(
        &self,
        query: crate::memtree_model::MemTreeQuery,
    ) -> Result<Vec<crate::memtree_model::MemTreeNodeRecord>, AmError>;
}

/// Wraps any [`Store`] and runs each call on Tokio's blocking pool.
pub struct BlockingAsyncStore<S: Store + 'static> {
    inner: Arc<S>,
}

impl<S: Store + 'static> BlockingAsyncStore<S> {
    /// Wrap a sync store for async callers.
    #[must_use]
    pub fn new(inner: Arc<S>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<S: Store + 'static> AsyncStore for BlockingAsyncStore<S> {
    async fn delete_item(&self, bank_id: String, item_id: String) -> Result<bool, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.delete_item(&bank_id, &item_id))
            .await
            .expect("store blocking task")
    }

    async fn create_memory_item(&self, item: MemoryItem, actor: String) -> Result<(), AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.create_memory_item(&item, &actor))
            .await
            .expect("store blocking task")
    }

    async fn update_memory_item(&self, item: MemoryItem, actor: String) -> Result<(), AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.update_memory_item(&item, &actor))
            .await
            .expect("store blocking task")
    }

    async fn get_memory_item(
        &self,
        bank_id: String,
        item_id: String,
    ) -> Result<Option<MemoryItem>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.get_memory_item(&bank_id, &item_id))
            .await
            .expect("store blocking task")
    }

    async fn get_snapshot(
        &self,
        bank_id: String,
        item_id: String,
    ) -> Result<Option<Value>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.get_snapshot(&bank_id, &item_id))
            .await
            .expect("store blocking task")
    }

    async fn get_gist(&self, bank_id: String, item_id: String) -> Result<Option<String>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.get_gist(&bank_id, &item_id))
            .await
            .expect("store blocking task")
    }

    async fn put_assoc_edge(&self, edge: AssocEdgeRecord) -> Result<(), AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.put_assoc_edge(&edge))
            .await
            .expect("store blocking task")
    }

    async fn get_assoc_edge(
        &self,
        bank_id: String,
        edge_id: String,
    ) -> Result<Option<AssocEdgeRecord>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.get_assoc_edge(&bank_id, &edge_id))
            .await
            .expect("store blocking task")
    }

    async fn delete_assoc_edge(&self, bank_id: String, edge_id: String) -> Result<bool, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.delete_assoc_edge(&bank_id, &edge_id))
            .await
            .expect("store blocking task")
    }

    async fn list_assoc_edges(
        &self,
        bank_id: String,
        item_id: Option<String>,
    ) -> Result<Vec<AssocEdgeRecord>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.list_assoc_edges(&bank_id, item_id.as_deref()))
            .await
            .expect("store blocking task")
    }

    async fn reinforce_assoc_edge(
        &self,
        bank_id: String,
        a: String,
        b: String,
        now_millis: i64,
        eta: f64,
    ) -> Result<AssocEdgeRecord, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            inner.reinforce_assoc_edge(&bank_id, &a, &b, now_millis, eta)
        })
        .await
        .expect("store blocking task")
    }

    async fn record_assoc_audit(
        &self,
        bank_id: String,
        operation: String,
        item_id: Option<String>,
        actor: String,
        detail: Value,
    ) -> Result<(), AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            inner.record_assoc_audit(&bank_id, &operation, item_id.as_deref(), &actor, &detail)
        })
        .await
        .expect("store blocking task")
    }

    async fn upsert_vector(&self, slot: VectorSlot) -> Result<(), AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.upsert_vector(&slot))
            .await
            .expect("store blocking task")
    }

    async fn delete_vector(&self, bank_id: String, item_id: String) -> Result<bool, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.delete_vector(&bank_id, &item_id))
            .await
            .expect("store blocking task")
    }

    async fn knn(
        &self,
        bank_id: String,
        query: Vec<f32>,
        k: usize,
    ) -> Result<Vec<KnnHit>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.knn(&bank_id, &query, k))
            .await
            .expect("store blocking task")
    }

    async fn upsert_subject_key_ref(
        &self,
        subject_id: String,
        dek_ref: String,
        status: String,
    ) -> Result<(), AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            inner.upsert_subject_key_ref(&subject_id, &dek_ref, &status)
        })
        .await
        .expect("store blocking task")
    }

    async fn inspect_content_ciphertext(
        &self,
        bank_id: String,
        item_id: String,
    ) -> Result<Option<String>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.inspect_content_ciphertext(&bank_id, &item_id))
            .await
            .expect("store blocking task")
    }

    async fn upsert_memtree_node(
        &self,
        node: crate::memtree_model::MemTreeNodeRecord,
    ) -> Result<(), AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.upsert_memtree_node(&node))
            .await
            .expect("store blocking task")
    }

    async fn get_memtree_node(
        &self,
        bank_id: String,
        node_id: String,
    ) -> Result<Option<crate::memtree_model::MemTreeNodeRecord>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.get_memtree_node(&bank_id, &node_id))
            .await
            .expect("store blocking task")
    }

    async fn query_memtree_nodes(
        &self,
        query: crate::memtree_model::MemTreeQuery,
    ) -> Result<Vec<crate::memtree_model::MemTreeNodeRecord>, AmError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.query_memtree_nodes(&query))
            .await
            .expect("store blocking task")
    }
}

#[cfg(test)]
#[path = "async_store_tests.rs"]
mod async_store_tests;
