// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite atomic batch executor: committing
//! every pre-validated batch write inside ONE transaction with drop-rollback.
//!
//! ## Owns
//! - `BatchStore for SqliteStore` (all-or-nothing commit)
//!
//! ## Does not own
//! - Gating/`$ref` resolution (callers stage admitted payloads)
//! - The per-op SQL (tx-scoped helpers in the domain modules)
//!
//! ## Boundary
//!
//! Changes to batch commit ordering or rollback behavior should normally
//! require changes here. Changes to single-op SQL should not require changes here.
//!
//! Keep this module focused on the SQLite batch executor.

use crate::batch_model::{BatchStore, BatchWrite, MAX_BATCH_OPS};
use crate::belief_store::{seal_belief_context, seal_proposition};
use crate::belief_tx::{
    append_confidence_sqlite_tx, create_belief_sqlite_tx, validate_append, validate_belief_create,
};
use crate::history_store::{seal_history_text, seal_steps};
use crate::item_persist::{require_actor, seal_dual};
use crate::item_update::update_memory_item_sqlite_tx;
use crate::migrate::IntoAmError;
use crate::opaque_item_persist::put_opaque_item_sqlite_tx;
use crate::persona_store::{seal_persona_context, seal_persona_value};
use crate::sqlite::SqliteStore;
use crate::sqlite_assoc::{put_assoc_edge_tx, record_audit_event_tx};
use crate::sqlite_history_write::{commit_failure_record_tx, commit_task_upsert_tx};
use crate::sqlite_memory::{active_item_id_by_subject_tx, create_memory_item_tx};
use crate::sqlite_persona::{put_preference_state_tx_audited, put_stable_entry_tx_audited};
use crate::sqlite_triple::commit_triple_add_tx;
use crate::sqlite_triple_invalidate::invalidate_item_tx;
use clio_types::{AmError, ErrorCode, proposition_key};

impl SqliteStore {
    /// Validate + seal an admitted carrier item (batch prelude).
    fn seal_item(&self, item: &clio_types::MemoryItem) -> Result<(String, String), AmError> {
        item.validate()?;
        let dek_ref = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        Ok((cipher, dek_ref))
    }
}

impl BatchStore for SqliteStore {
    fn apply_batch(&self, writes: &[BatchWrite]) -> Result<(), AmError> {
        if writes.len() > MAX_BATCH_OPS {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("batch exceeds the {MAX_BATCH_OPS}-operation cap"),
            ));
        }
        self.with_conn(|conn| {
            let tx = conn.transaction().am("batch begin")?;
            for write in writes {
                apply_one(self, &tx, write)?;
            }
            tx.commit().am("batch commit")?;
            Ok(())
        })
    }
}

/// Execute one pre-validated write inside the open batch transaction.
#[allow(clippy::too_many_lines)]
fn apply_one(
    store: &SqliteStore,
    tx: &rusqlite::Transaction<'_>,
    write: &BatchWrite,
) -> Result<(), AmError> {
    match write {
        BatchWrite::CreateItem { item, actor } => {
            let (cipher, dek_ref) = store.seal_item(item)?;
            create_memory_item_tx(tx, item, &cipher, &dek_ref, actor)
        }
        BatchWrite::CanonicalPut { item, key, actor } => {
            let prior = active_item_id_by_subject_tx(tx, &item.bank_id, &item.subject_id)?
                .filter(|prior| prior != &item.id);
            if let Some(prior_id) = &prior {
                invalidate_item_tx(
                    tx,
                    &item.bank_id,
                    prior_id,
                    &item.valid_from,
                    &item.tx_from,
                    Some(&item.id),
                    actor,
                )?;
            }
            let (cipher, dek_ref) = store.seal_item(item)?;
            create_memory_item_tx(tx, item, &cipher, &dek_ref, actor)?;
            record_audit_event_tx(
                tx,
                &item.bank_id,
                "canonical_put",
                Some(&item.id),
                actor,
                &serde_json::json!({
                    "key": key,
                    "superseded_item_id": prior,
                    "admission_score": item.admission_score
                }),
            )
        }
        BatchWrite::CommitTriple {
            item,
            edge,
            supersede,
            actor,
        } => {
            item.validate()?;
            edge.validate()?;
            if item.id != edge.item_id || item.bank_id != edge.bank_id {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "triple item_id/bank_id must match carrier MemoryItem",
                ));
            }
            if item.episodic_type != Some(clio_types::EpisodicType::Triple) {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "triple carrier item must have episodic_type=triple",
                ));
            }
            let (cipher, dek_ref) = store.seal_item(item)?;
            let object_cipher = crate::triple_cipher::seal_object(
                store.kms().as_ref(),
                &item.subject_id,
                &edge.object,
            )?;
            let seal = crate::triple_cipher::TripleSeal {
                carrier: &cipher,
                object: &object_cipher,
                dek_ref: &dek_ref,
            };
            commit_triple_add_tx(tx, item, edge, *supersede, actor, &seal)?;
            Ok(())
        }
        BatchWrite::CloseItem {
            bank_id,
            item_id,
            valid_until,
            tx_until,
            replacement_id,
            actor,
        } => {
            invalidate_item_tx(
                tx,
                bank_id,
                item_id,
                valid_until,
                tx_until,
                replacement_id.as_deref(),
                actor,
            )?;
            Ok(())
        }
        BatchWrite::CommitTaskUpsert {
            item,
            record,
            actor,
        } => {
            let (cipher, dek_ref) = store.seal_item(item)?;
            let definition =
                seal_history_text(store.kms().as_ref(), &item.subject_id, &record.definition)?;
            let steps = seal_steps(store.kms().as_ref(), &item.subject_id, &record.steps)?;
            commit_task_upsert_tx(
                tx,
                item,
                record,
                actor,
                &cipher,
                &dek_ref,
                &definition,
                &steps,
            )?;
            Ok(())
        }
        BatchWrite::CommitFailureRecord {
            item,
            record,
            actor,
        } => {
            let (cipher, dek_ref) = store.seal_item(item)?;
            let what_failed =
                seal_history_text(store.kms().as_ref(), &item.subject_id, &record.what_failed)?;
            let lesson = seal_history_text(store.kms().as_ref(), &item.subject_id, &record.lesson)?;
            commit_failure_record_tx(
                tx,
                item,
                record,
                actor,
                &cipher,
                &dek_ref,
                &what_failed,
                &lesson,
            )?;
            Ok(())
        }
        BatchWrite::PutStableEntry { entry, actor } => {
            entry.validate()?;
            require_actor(actor)?;
            let dek_ref = store.kms().ensure_dek(&entry.bank_id)?;
            let cipher = seal_persona_value(store.kms().as_ref(), &entry.bank_id, &entry.value)?;
            let context_cipher = seal_persona_context(
                store.kms().as_ref(),
                &entry.bank_id,
                entry.context.as_deref(),
            )?;
            put_stable_entry_tx_audited(
                tx,
                store.kms().as_ref(),
                entry,
                &cipher,
                &dek_ref,
                context_cipher.as_deref(),
                actor,
            )
        }
        BatchWrite::PutAssocEdge { edge, .. } => {
            if edge.id.is_empty() || edge.bank_id.is_empty() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "assoc edge id and bank_id are required",
                ));
            }
            if !(0.0..=1.0).contains(&edge.weight) {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    "assoc edge weight must be in 0.0..=1.0",
                ));
            }
            put_assoc_edge_tx(tx, edge)
        }
        BatchWrite::UpsertMemoryItem { item, actor } => {
            let (cipher, _dek_ref) = store.seal_item(item)?;
            update_memory_item_sqlite_tx(tx, item, &cipher, actor)
        }
        BatchWrite::PutOpaqueItem { item, .. } => put_opaque_item_sqlite_tx(tx, item),
        BatchWrite::PutPreferenceState { preference, actor } => {
            preference.validate()?;
            require_actor(actor)?;
            let context_cipher = seal_persona_context(
                store.kms().as_ref(),
                &preference.bank_id,
                preference.context.as_deref(),
            )?;
            put_preference_state_tx_audited(
                tx,
                store.kms().as_ref(),
                preference,
                context_cipher.as_deref(),
                actor,
            )
        }
        BatchWrite::CreateBelief { belief, actor } => {
            validate_belief_create(belief, actor)?;
            let dek_ref = store.kms().ensure_dek(&belief.subject_id)?;
            let cipher = seal_proposition(
                store.kms().as_ref(),
                &belief.subject_id,
                &belief.proposition,
            )?;
            let context_cipher = seal_belief_context(
                store.kms().as_ref(),
                &belief.subject_id,
                belief.context.as_deref(),
            )?;
            let key = proposition_key(&belief.proposition);
            create_belief_sqlite_tx(
                tx,
                belief,
                &cipher,
                context_cipher.as_deref(),
                &dek_ref,
                &key,
                actor,
            )
        }
        BatchWrite::AppendBeliefConfidence {
            bank_id,
            belief_id,
            entry,
            actor,
        } => {
            validate_append(bank_id, entry, actor)?;
            append_confidence_sqlite_tx(tx, bank_id, belief_id, entry, actor, None, None)
        }
        BatchWrite::AuditEvent {
            bank_id,
            operation,
            item_id,
            actor,
            detail,
        } => record_audit_event_tx(tx, bank_id, operation, item_id.as_deref(), actor, detail),
    }
}
