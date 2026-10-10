// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite history writes: gated carrier
//! items plus structured task/failure rows committed atomically.
//!
//! ## Owns
//! - `commit_task_upsert` / `commit_failure_record` SQLite transactions
//!
//! ## Does not own
//! - History reads (see `sqlite_history_read`) or the `HistoryStore` contract
//!
//! ## Boundary
//!
//! Changes to SQLite history write SQL should normally require changes here.
//! Changes to history reads should not require changes here.

use crate::history_store::{
    history_audit_detail, require_nonempty, seal_history_text, seal_steps,
    validate_failure_carrier, validate_task_carrier,
};
use crate::item_persist::{
    kind_columns, map_item_insert, require_actor, seal_dual, source_type_sql,
};
use crate::migrate::{IntoAmError, now_iso};
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode, FailureRecord, MemoryItem, TaskRecord};
use rusqlite::{Transaction, params};

/// Map a SQLite history insert: unique violations become structured conflicts.
fn map_history_insert(result: Result<u64, rusqlite::Error>) -> Result<(), AmError> {
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("UNIQUE") || msg.contains("unique") || msg.contains("duplicate key") {
                Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "history record already exists for this key; retry with a new version/attempt",
                ))
            } else {
                Err(AmError::new(
                    ErrorCode::Internal,
                    format!("history insert: {msg}"),
                ))
            }
        }
    }
}

/// Upsert `subject_keys` and insert the encrypted carrier item row.
fn insert_carrier(
    tx: &Transaction<'_>,
    item: &MemoryItem,
    cipher: &str,
    dek_ref: &str,
    ts: &str,
) -> Result<(), AmError> {
    let sk_rows = tx
        .execute(
            "INSERT INTO subject_keys (subject_id, dek_ref, status, created_at, updated_at)
         VALUES (?1, ?2, 'active', ?3, ?3)
         ON CONFLICT(subject_id) DO UPDATE SET
           dek_ref = excluded.dek_ref,
           status = 'active',
           shredded_at = NULL,
           updated_at = excluded.updated_at
           WHERE subject_keys.shredded_at IS NULL",
            params![item.subject_id, dek_ref, ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows as u64, &item.subject_id)?;
    let (category, episodic_type) = kind_columns(item);
    let source_type = source_type_sql(item);
    map_item_insert(
        tx.execute(
            "INSERT INTO items (
                id, bank_id, kind, category, episodic_type, epistemic_kind, source_type,
                confidence, admission_score, source_ref, content_ciphertext, subject_id,
                valid_from, valid_until, tx_from, tx_until, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18
             )",
            params![
                item.id,
                item.bank_id,
                item.kind.as_str(),
                category,
                episodic_type,
                item.epistemic_kind.as_str(),
                source_type,
                item.confidence,
                item.admission_score,
                item.source_ref,
                cipher,
                item.subject_id,
                item.valid_from,
                item.valid_until,
                item.tx_from,
                item.tx_until,
                item.created_at,
                item.updated_at,
            ],
        )
        .map(|n| n as u64)
        .map_err(|e| e.to_string()),
    )
}

/// Transaction-scoped task upsert: version bump + carrier + row + audit +
/// index enqueue. Shared by the single-op path and the batch executor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_task_upsert_tx(
    tx: &Transaction<'_>,
    item: &MemoryItem,
    draft: &TaskRecord,
    actor: &str,
    cipher: &str,
    dek_ref: &str,
    definition: &str,
    steps: &str,
) -> Result<i64, AmError> {
    // The batch executor reaches this helper directly, so the structured-record
    // contract is enforced here as well as on the single-op entry point
    // A malformed draft context never reaches sealing or SQL.
    draft.validate()?;
    validate_task_carrier(item, draft)?;
    let ts = now_iso();
    let version: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM task_records
             WHERE bank_id = ?1 AND task_id = ?2",
            params![item.bank_id, draft.task_id],
            |r| r.get(0),
        )
        .am("task next version")?;
    insert_carrier(tx, item, cipher, dek_ref, &ts)?;
    map_history_insert(
        tx.execute(
            "INSERT INTO task_records (
                id, bank_id, item_id, task_id, subject_id, version, status,
                definition_cipher, steps_cipher, evidence_ref, admission_score, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                draft.id,
                item.bank_id,
                item.id,
                draft.task_id,
                item.subject_id,
                version,
                draft.status,
                definition,
                steps,
                draft.evidence_ref,
                item.admission_score.unwrap_or(0.0),
                item.created_at,
            ],
        )
        .map(|n| n as u64),
    )?;
    let detail = history_audit_detail(item, "task", &draft.task_id, version, draft.steps.len());
    let (category, _) = kind_columns(item);
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES (?1, ?2, 'task_upsert', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            format!("audit-{}-{ts}", item.id),
            item.bank_id,
            item.id,
            category,
            item.epistemic_kind.as_str(),
            item.confidence,
            item.admission_score,
            actor,
            detail,
            ts,
        ],
    )
    .am("task audit insert")?;
    crate::sqlite_index::enqueue_index_pending_tx(tx, &item.bank_id, &item.id)?;
    Ok(version)
}

/// Transaction-scoped failure record insert: carrier + row + audit + enqueue.
#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_failure_record_tx(
    tx: &Transaction<'_>,
    item: &MemoryItem,
    draft: &FailureRecord,
    actor: &str,
    cipher: &str,
    dek_ref: &str,
    what_failed: &str,
    lesson: &str,
) -> Result<(), AmError> {
    // Same fail-closed contract as the task helper: the batch executor calls
    // this directly.
    draft.validate()?;
    validate_failure_carrier(item, draft)?;
    let ts = now_iso();
    insert_carrier(tx, item, cipher, dek_ref, &ts)?;
    map_history_insert(
        tx.execute(
            "INSERT INTO failure_records (
                id, bank_id, item_id, task_id, subject_id, attempt_n, what_failed_cipher,
                lesson_cipher, lesson_tokens, evidence_ref, admission_score,
                epistemic_kind, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                draft.id,
                item.bank_id,
                item.id,
                draft.task_id,
                item.subject_id,
                draft.attempt_n,
                what_failed,
                lesson,
                draft.lesson_tokens,
                draft.evidence_ref,
                item.admission_score.unwrap_or(0.0),
                item.epistemic_kind.as_str(),
                item.created_at,
            ],
        )
        .map(|n| n as u64),
    )?;
    let (category, _) = kind_columns(item);
    let detail = history_audit_detail(item, "failure", &draft.task_id, draft.attempt_n, 0);
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES (?1, ?2, 'failure_record', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            format!("audit-{}-{ts}", item.id),
            item.bank_id,
            item.id,
            category,
            item.epistemic_kind.as_str(),
            item.confidence,
            item.admission_score,
            actor,
            detail,
            ts,
        ],
    )
    .am("failure audit insert")?;
    crate::sqlite_index::enqueue_index_pending_tx(tx, &item.bank_id, &item.id)?;
    Ok(())
}

impl SqliteStore {
    pub(crate) fn commit_task_upsert_impl(
        &self,
        item: &MemoryItem,
        draft: &TaskRecord,
        actor: &str,
    ) -> Result<TaskRecord, AmError> {
        item.validate()?;
        draft.validate()?;
        require_actor(actor)?;
        validate_task_carrier(item, draft)?;
        require_nonempty(&item.bank_id, "bank_id")?;
        require_nonempty(&draft.task_id, "task_id")?;
        let dek_ref = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        let definition =
            seal_history_text(self.kms().as_ref(), &item.subject_id, &draft.definition)?;
        let steps = seal_steps(self.kms().as_ref(), &item.subject_id, &draft.steps)?;
        let version = self.with_conn(|conn| {
            let tx = conn.transaction().am("task upsert begin")?;
            let version = commit_task_upsert_tx(
                &tx,
                item,
                draft,
                actor,
                &cipher,
                &dek_ref,
                &definition,
                &steps,
            )?;
            tx.commit().am("task upsert commit")?;
            Ok(version)
        })?;
        let mut stored = draft.clone();
        stored.version = version;
        stored.admission_score = item.admission_score.unwrap_or(0.0);
        stored.created_at.clone_from(&item.created_at);
        Ok(stored)
    }

    pub(crate) fn commit_failure_record_impl(
        &self,
        item: &MemoryItem,
        draft: &FailureRecord,
        actor: &str,
    ) -> Result<FailureRecord, AmError> {
        item.validate()?;
        draft.validate()?;
        require_actor(actor)?;
        validate_failure_carrier(item, draft)?;
        let dek_ref = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        let what_failed =
            seal_history_text(self.kms().as_ref(), &item.subject_id, &draft.what_failed)?;
        let lesson = seal_history_text(self.kms().as_ref(), &item.subject_id, &draft.lesson)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("failure record begin")?;
            commit_failure_record_tx(
                &tx,
                item,
                draft,
                actor,
                &cipher,
                &dek_ref,
                &what_failed,
                &lesson,
            )?;
            tx.commit().am("failure record commit")?;
            Ok(())
        })?;
        let mut stored = draft.clone();
        stored.admission_score = item.admission_score.unwrap_or(0.0);
        stored.created_at.clone_from(&item.created_at);
        Ok(stored)
    }
}
