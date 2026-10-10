// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite `Store` trait implementation surface.
//!
//! ## Owns
//! - Envelope, assoc, vector, memtree, and triple Store method wiring
//!
//! ## Does not own
//! - Connection open/migrate (see `sqlite`), the continuous-attribute
//!   `ContinuousStore` impl (see `sqlite_attribute`), or SQL bodies elsewhere
//!
//! ## Boundary
//!
//! Changes to which Store methods SQLite exposes should normally require changes here.

use crate::distilled_unit::DistilledUnitRecord;
use crate::embedding_space::EmbeddingSpace;
use crate::migrate::{IntoAmError, now_iso};
use crate::model::{AssocEdgeRecord, KnnHit, VectorSlot};
use crate::sqlite::SqliteStore;
use crate::sqlite_knn::knn_in_bank;
use crate::store::Store;
use clio_types::{AmError, MemoryItem};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use zerocopy::IntoBytes;

impl Store for SqliteStore {
    fn delete_item(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        self.delete_item_impl(bank_id, item_id)
    }

    fn create_memory_item(
        &self,
        item: &clio_types::MemoryItem,
        actor: &str,
    ) -> Result<(), AmError> {
        self.create_memory_item_impl(item, actor)
    }

    fn update_memory_item(
        &self,
        item: &clio_types::MemoryItem,
        actor: &str,
    ) -> Result<(), AmError> {
        self.update_memory_item_impl(item, actor)
    }

    fn get_memory_item(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<clio_types::MemoryItem>, AmError> {
        self.get_memory_item_impl(bank_id, item_id)
    }

    fn get_snapshot(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<serde_json::Value>, AmError> {
        self.get_snapshot_impl(bank_id, item_id)
    }

    fn get_gist(&self, bank_id: &str, item_id: &str) -> Result<Option<String>, AmError> {
        self.get_gist_impl(bank_id, item_id)
    }

    fn put_assoc_edge(&self, edge: &AssocEdgeRecord) -> Result<(), AmError> {
        self.put_assoc_edge_impl(edge)
    }

    fn get_assoc_edge(
        &self,
        bank_id: &str,
        edge_id: &str,
    ) -> Result<Option<AssocEdgeRecord>, AmError> {
        self.get_assoc_edge_impl(bank_id, edge_id)
    }

    fn delete_assoc_edge(&self, bank_id: &str, edge_id: &str) -> Result<bool, AmError> {
        self.delete_assoc_edge_impl(bank_id, edge_id)
    }

    fn list_assoc_edges(
        &self,
        bank_id: &str,
        item_id: Option<&str>,
    ) -> Result<Vec<AssocEdgeRecord>, AmError> {
        self.list_assoc_edges_impl(bank_id, item_id)
    }

    fn reinforce_assoc_edge(
        &self,
        bank_id: &str,
        a: &str,
        b: &str,
        now_millis: i64,
        eta: f64,
    ) -> Result<AssocEdgeRecord, AmError> {
        self.reinforce_assoc_edge_impl(bank_id, a, b, now_millis, eta)
    }

    fn record_assoc_audit(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError> {
        self.record_assoc_audit_impl(bank_id, operation, item_id, actor, detail)
    }

    fn record_audit_event(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError> {
        self.record_audit_event_impl(bank_id, operation, item_id, actor, detail)
    }

    fn get_item_by_subject(
        &self,
        bank_id: &str,
        subject_id: &str,
        as_of: Option<&str>,
    ) -> Result<Option<MemoryItem>, AmError> {
        self.get_item_by_subject_impl(bank_id, subject_id, as_of)
    }

    fn upsert_vector(&self, slot: &VectorSlot) -> Result<(), AmError> {
        self.validate_dims(&slot.embedding)?;
        self.validate_model(&slot.model_id)?;
        let ts = now_iso();
        self.with_conn(|conn| {
            // vec0 upsert: delete then insert (virtual table).
            conn.execute(
                "DELETE FROM item_embeddings WHERE item_id = ?1",
                params![slot.item_id],
            )
            .am("vector delete-before-upsert")?;
            conn.execute(
                "INSERT INTO item_embeddings (item_id, embedding, bank_id, model_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![
                    slot.item_id,
                    slot.embedding.as_bytes(),
                    slot.bank_id,
                    slot.model_id,
                    ts,
                ],
            )
            .am("vector insert")?;
            Ok(())
        })
    }

    fn delete_vector(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM item_embeddings WHERE item_id = ?1 AND bank_id = ?2",
                    params![item_id, bank_id],
                )
                .am("vector delete")?;
            Ok(n > 0)
        })
    }

    fn knn(&self, bank_id: &str, query: &[f32], k: usize) -> Result<Vec<KnnHit>, AmError> {
        self.validate_dims(query)?;
        if k == 0 {
            return Ok(Vec::new());
        }
        self.with_conn(|conn| knn_in_bank(conn, bank_id, query, k))
    }

    fn embedding_space(&self) -> Result<EmbeddingSpace, AmError> {
        Ok(self.space().clone())
    }

    fn upsert_subject_key_ref(
        &self,
        subject_id: &str,
        dek_ref: &str,
        status: &str,
    ) -> Result<(), AmError> {
        let ts = now_iso();
        let shredded = if status == "shredded" {
            Some(ts.clone())
        } else {
            None
        };
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO subject_keys (subject_id, dek_ref, status, shredded_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(subject_id) DO UPDATE SET
                   dek_ref = excluded.dek_ref,
                   status = excluded.status,
                   shredded_at = excluded.shredded_at,
                   updated_at = excluded.updated_at",
                params![subject_id, dek_ref, status, shredded, ts],
            )
            .am("subject_keys")?;
            Ok(())
        })
    }

    fn inspect_content_ciphertext(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<String>, AmError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT content_ciphertext FROM items WHERE id = ?1 AND bank_id = ?2",
                params![item_id, bank_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .am("inspect ciphertext")
            .map(|opt| opt.flatten())
        })
    }

    fn upsert_memtree_node(
        &self,
        node: &crate::memtree_model::MemTreeNodeRecord,
    ) -> Result<(), AmError> {
        self.with_conn(|conn| crate::sqlite_memtree::upsert(conn, node))
    }

    fn get_memtree_node(
        &self,
        bank_id: &str,
        node_id: &str,
    ) -> Result<Option<crate::memtree_model::MemTreeNodeRecord>, AmError> {
        self.with_conn(|conn| crate::sqlite_memtree::get(conn, bank_id, node_id))
    }

    fn query_memtree_nodes(
        &self,
        query: &crate::memtree_model::MemTreeQuery,
    ) -> Result<Vec<crate::memtree_model::MemTreeNodeRecord>, AmError> {
        self.with_conn(|conn| crate::sqlite_memtree::query(conn, query))
    }

    fn commit_triple_add(
        &self,
        item: &clio_types::MemoryItem,
        edge: &clio_types::TripleRecord,
        supersede: bool,
        actor: &str,
    ) -> Result<clio_types::TripleAddOutcome, AmError> {
        self.commit_triple_add_impl(item, edge, supersede, actor)
    }

    fn get_triple(
        &self,
        bank_id: &str,
        edge_id: &str,
    ) -> Result<Option<clio_types::TripleRecord>, AmError> {
        self.get_triple_impl(bank_id, edge_id)
    }

    fn query_triples(
        &self,
        query: &clio_types::TripleQuery,
    ) -> Result<Vec<clio_types::TripleRecord>, AmError> {
        self.query_triples_impl(query)
    }

    fn list_triples(&self, bank_id: &str) -> Result<Vec<clio_types::TripleRecord>, AmError> {
        self.list_triples_impl(bank_id)
    }

    fn commit_distilled_unit(
        &self,
        item: &MemoryItem,
        record: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<(), AmError> {
        self.commit_distilled_unit_impl(item, record, actor)
    }

    fn get_distilled_unit(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<DistilledUnitRecord>, AmError> {
        self.get_distilled_unit_impl(bank_id, item_id)
    }

    fn triple_end(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        object: Option<&str>,
        valid_until: &str,
        tx_until: &str,
    ) -> Result<clio_types::TripleEndOutcome, AmError> {
        self.triple_end_impl(bank_id, subject, predicate, object, valid_until, tx_until)
    }

    fn invalidate_item(
        &self,
        bank_id: &str,
        item_id: &str,
        valid_until: &str,
        tx_until: &str,
        replacement_id: Option<&str>,
        actor: &str,
    ) -> Result<clio_types::TripleEndOutcome, AmError> {
        self.invalidate_item_impl(
            bank_id,
            item_id,
            valid_until,
            tx_until,
            replacement_id,
            actor,
        )
    }

    fn get_item_identity(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<crate::model::ItemIdentity>, AmError> {
        self.get_item_identity_impl(bank_id, item_id)
    }

    fn discard_item(
        &self,
        bank_id: &str,
        item_id: &str,
        reason: &str,
        discarded_at: &str,
        actor: &str,
    ) -> Result<crate::model::ItemIdentity, AmError> {
        self.discard_item_impl(bank_id, item_id, reason, discarded_at, actor)
    }

    fn item_is_archived(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        self.item_is_archived_impl(bank_id, item_id)
    }

    fn item_ids_in_window(
        &self,
        bank_id: &str,
        axis: clio_types::TimeAxis,
        start: &str,
        end: &str,
        limit: usize,
    ) -> Result<Vec<String>, AmError> {
        self.item_ids_in_window_impl(bank_id, axis, start, end, limit)
    }

    fn inspect_items(
        &self,
        filter: &crate::model::ItemInspectFilter,
    ) -> Result<Vec<crate::model::ItemInspectRow>, AmError> {
        self.inspect_items_impl(filter)
    }

    fn count_items(&self, filter: &crate::model::ItemInspectFilter) -> Result<usize, AmError> {
        self.count_items_impl(filter)
    }

    fn correct_item(
        &self,
        spec: &crate::audit_model::CorrectionSpec,
    ) -> Result<crate::audit_model::CorrectionOutcome, AmError> {
        self.correct_item_impl(spec)
    }

    fn audit_trail(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<crate::audit_model::ItemAuditTrail, AmError> {
        self.audit_trail_impl(bank_id, item_id)
    }
}

impl crate::triple_assertion::TripleAssertion for SqliteStore {
    fn triple_asserted_ever(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        object: Option<&str>,
    ) -> Result<bool, AmError> {
        self.triple_asserted_ever_impl(bank_id, subject, predicate, object)
    }
}
