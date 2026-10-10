// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the transaction-scoped item-field update
//! shared by both backends: mutable-field UPDATE, update audit row, and index
//! enqueue inside an already-open transaction.
//!
//! ## Owns
//! - `update_memory_item_sqlite_tx` / `update_memory_item_postgres_tx`
//! - The update-path subject-immutability check (move refusal + NULL
//!   stored-subject named error)
//!
//! ## Does not own
//! - Item validation or sealing (callers seal before calling)
//! - The single-op `update_memory_item` wrapper (see the per-backend memory modules)
//!
//! ## Boundary
//!
//! Changes to the mutable-field UPDATE SQL should normally require changes
//! here. Changes to item creation SQL should not.
//!
//! Keep this module focused on the item update transaction body.

use crate::item_persist::source_type_sql;
use crate::migrate::{IntoAmError, now_iso};
use clio_types::{AmError, ErrorCode, MemoryItem};
use rusqlite::OptionalExtension;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide monotonic suffix for update audit ids. Two updates of the same
/// item within the same millisecond would otherwise produce the same
/// `audit_events.id` primary key and fail the insert.
static AUDIT_SEQ: AtomicU64 = AtomicU64::new(0);

fn update_audit_id(item_id: &str, ts: &str) -> String {
    let seq = AUDIT_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("audit-upd-{item_id}-{ts}-{seq}")
}

/// Fail closed on a stored row whose `subject_id` is NULL (legacy or
/// hand-written data): the stored subject is the immutability anchor, so it
/// cannot be compared and the update must not proceed.
fn null_subject_err(item_id: &str) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "item `{item_id}` has NULL subject_id; cannot enforce subject \
             immutability — repair the stored row before updating"
        ),
    )
}

/// Apply the mutable-field update + audit + index enqueue on SQLite.
pub(crate) fn update_memory_item_sqlite_tx(
    tx: &rusqlite::Transaction<'_>,
    item: &MemoryItem,
    cipher: &str,
    actor: &str,
) -> Result<(), AmError> {
    use rusqlite::params;
    let source_type = source_type_sql(item);
    let ts = now_iso();
    let audit_id = update_audit_id(&item.id, &ts);
    // The subject anchors the DEK and erasure scope: updates may not move an
    // item to a different subject. Read as Option<String> so a NULL stored
    // subject (legacy data) is a named error, not a driver error.
    let stored: Option<String> = tx
        .query_row(
            "SELECT subject_id FROM items WHERE id = ?1 AND bank_id = ?2",
            params![item.id, item.bank_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .am("subject lookup")?
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "item not found for update"))?;
    let Some(stored) = stored else {
        return Err(null_subject_err(&item.id));
    };
    if stored != item.subject_id {
        return Err(crate::item_persist::subject_immutable_err(&item.id));
    }
    let n = tx
        .execute(
            "UPDATE items SET
                epistemic_kind = ?1,
                source_type = ?2,
                confidence = ?3,
                admission_score = ?4,
                source_ref = ?5,
                content_ciphertext = ?6,
                subject_id = ?7,
                valid_until = ?8,
                tx_until = ?9,
                updated_at = ?10
             WHERE id = ?11 AND bank_id = ?12",
            params![
                item.epistemic_kind.as_str(),
                source_type,
                item.confidence,
                item.admission_score,
                item.source_ref,
                cipher,
                item.subject_id,
                item.valid_until,
                item.tx_until,
                item.updated_at,
                item.id,
                item.bank_id,
            ],
        )
        .am("items update")?;
    if n == 0 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item not found for update",
        ));
    }
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES (?1, ?2, 'update_memory_item', ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            audit_id,
            item.bank_id,
            item.id,
            item.epistemic_kind.as_str(),
            item.confidence,
            item.admission_score,
            actor,
            crate::item_persist::context_audit_detail(item),
            ts,
        ],
    )
    .am("audit update")?;
    crate::sqlite_index::enqueue_index_pending_tx(tx, &item.bank_id, &item.id)?;
    Ok(())
}

/// Apply the mutable-field update + audit + index enqueue on Postgres.
pub(crate) fn update_memory_item_postgres_tx(
    tx: &mut postgres::Transaction<'_>,
    item: &MemoryItem,
    cipher: &str,
    actor: &str,
) -> Result<(), AmError> {
    let source_type = source_type_sql(item).map(str::to_owned);
    let epistemic = item.epistemic_kind.as_str().to_owned();
    let ts = now_iso();
    let audit_id = update_audit_id(&item.id, &ts);
    // The subject anchors the DEK and erasure scope: updates may not move an
    // item to a different subject. Read as Option<String> so a NULL stored
    // subject (legacy data) is a named error, not a panic.
    let stored: Option<Option<String>> = tx
        .query_opt(
            "SELECT subject_id FROM items WHERE id = $1 AND bank_id = $2",
            &[&item.id, &item.bank_id],
        )
        .am("subject lookup")?
        .map(|row| row.get::<_, Option<String>>(0));
    let Some(stored) = stored else {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item not found for update",
        ));
    };
    let Some(stored) = stored else {
        return Err(null_subject_err(&item.id));
    };
    if stored != item.subject_id {
        return Err(crate::item_persist::subject_immutable_err(&item.id));
    }
    let n = tx
        .execute(
            "UPDATE items SET
                epistemic_kind = $1,
                source_type = $2,
                confidence = $3::float8,
                admission_score = $4::float8,
                source_ref = $5,
                content_ciphertext = $6,
                subject_id = $7,
                valid_until = $8,
                tx_until = $9,
                updated_at = $10
             WHERE id = $11 AND bank_id = $12",
            &[
                &epistemic,
                &source_type,
                &item.confidence,
                &item.admission_score,
                &item.source_ref,
                &cipher,
                &item.subject_id,
                &item.valid_until,
                &item.tx_until,
                &item.updated_at,
                &item.id,
                &item.bank_id,
            ],
        )
        .am("items update")?;
    if n == 0 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item not found for update",
        ));
    }
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES ($1, $2, 'update_memory_item', $3, NULL, $4, $5::float8, $6::float8, $7, $8, $9)",
        &[
            &audit_id,
            &item.bank_id,
            &item.id,
            &epistemic,
            &item.confidence,
            &item.admission_score,
            &actor,
            &crate::item_persist::context_audit_detail(item),
            &ts,
        ],
    )
    .am("audit update")?;
    crate::postgres_index::enqueue_index_pending_tx(tx, &item.bank_id, &item.id)?;
    Ok(())
}
