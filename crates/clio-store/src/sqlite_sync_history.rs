// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite history sync SQL bodies
//! behind the `SyncStore` impl: the task/failure change feeds and the
//! apply read/write primitives (`task_current`, `task_put`,
//! `failure_by_attempt`, `failure_put`), materialized through the existing
//! atomic history commit paths.
//!
//! ## Owns
//! - SQLite SQL bodies for the task/failure sync feeds and apply primitives
//!
//! ## Does not own
//! - The `SyncStore for SqliteStore` impl block (`sqlite_sync`), which
//!   delegates to these bodies
//! - Item / assoc / triple / persona / belief feed SQL (`sync_records`)
//! - Postgres history sync persistence (`postgres_sync_history`)
//!
//! ## Boundary
//!
//! Changes to SQLite task/failure sync SQL should normally require changes
//! here. Changes to apply or conflict semantics should not.
//!
//! Keep this module focused on SQLite history sync persistence.

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::sync_store_rows::split_push_watermark;
use clio_types::{AmError, FailureRecord, TaskRecord};
use rusqlite::params;

/// Change-feed SQL: task-record versions changed past a watermark (compound
/// `(created_at, record id)` resume; the record id, not `task_id`, breaks
/// ties because several versions share one task id).
const FEED_TASKS_SQL: &str = "SELECT t.id, t.item_id, t.task_id, t.subject_id, t.version, \
  t.status, t.definition_cipher, t.steps_cipher, t.evidence_ref, t.admission_score, t.created_at \
  FROM task_records t WHERE t.bank_id = ?1 \
  AND (?2 = '1970-01-01T00:00:00Z' OR t.created_at > ?2 \
  OR (t.created_at = ?2 AND t.id > ?3)) \
  ORDER BY t.created_at, t.id LIMIT ?4";

/// Change-feed SQL: failure records changed past a watermark (compound
/// `(created_at, failure id)` resume).
const FEED_FAILURES_SQL: &str = "SELECT f.id, f.item_id, f.task_id, f.subject_id, f.attempt_n, \
  f.what_failed_cipher, f.lesson_cipher, f.lesson_tokens, f.evidence_ref, f.admission_score, \
  f.epistemic_kind, f.created_at FROM failure_records f WHERE f.bank_id = ?1 \
  AND (?2 = '1970-01-01T00:00:00Z' OR f.created_at > ?2 \
  OR (f.created_at = ?2 AND f.id > ?3)) \
  ORDER BY f.created_at, f.id LIMIT ?4";

/// Apply read SQL: one failure row by `(bank, task_id, attempt_n)`, joined to
/// its carrier item for the context.
const FAILURE_ATTEMPT_SQL: &str = "SELECT f.id, f.bank_id, f.item_id, f.task_id, f.subject_id, \
  f.attempt_n, f.what_failed_cipher, f.lesson_cipher, f.lesson_tokens, f.evidence_ref, \
  f.admission_score, f.epistemic_kind, f.created_at, i.content_ciphertext \
  FROM failure_records f LEFT JOIN items i ON i.id = f.item_id \
  WHERE f.bank_id = ?1 AND f.task_id = ?2 AND f.attempt_n = ?3 LIMIT 1";

/// SQLite body of `SyncStore::feed_tasks`: every task version whose change
/// stamp moved past the watermark, opened from its DEK-sealed columns.
pub(crate) fn feed_tasks_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, TaskRecord)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn.prepare(FEED_TASKS_SQL).am("sync feed tasks prepare")?;
        let rows = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, f64>(9)?,
                        r.get::<_, String>(10)?,
                    ))
                },
            )
            .am("sync feed tasks query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync feed tasks map")?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                item_id,
                task_id,
                subject_id,
                version,
                status,
                definition_cipher,
                steps_cipher,
                evidence_ref,
                admission_score,
                created_at,
            ) = row;
            out.push((
                created_at.clone(),
                TaskRecord {
                    id,
                    bank_id: bank_id.to_owned(),
                    item_id,
                    task_id,
                    subject_id,
                    version,
                    definition: crate::history_store::open_history_text(
                        store.kms().as_ref(),
                        &definition_cipher,
                    )?,
                    steps: crate::history_store::open_steps(store.kms().as_ref(), &steps_cipher)?,
                    status,
                    evidence_ref,
                    context: None,
                    admission_score,
                    created_at,
                },
            ));
        }
        Ok(out)
    })
}

/// SQLite body of `SyncStore::feed_failures`.
pub(crate) fn feed_failures_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, FailureRecord)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(FEED_FAILURES_SQL)
            .am("sync feed failures prepare")?;
        let rows = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, f64>(9)?,
                        r.get::<_, String>(10)?,
                        r.get::<_, String>(11)?,
                    ))
                },
            )
            .am("sync feed failures query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync feed failures map")?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                item_id,
                task_id,
                subject_id,
                attempt_n,
                what_failed_cipher,
                lesson_cipher,
                lesson_tokens,
                evidence_ref,
                admission_score,
                epistemic_kind,
                created_at,
            ) = row;
            out.push((
                created_at.clone(),
                FailureRecord {
                    id,
                    bank_id: bank_id.to_owned(),
                    item_id,
                    task_id,
                    subject_id,
                    attempt_n,
                    what_failed: crate::history_store::open_history_text(
                        store.kms().as_ref(),
                        &what_failed_cipher,
                    )?,
                    lesson: crate::history_store::open_history_text(
                        store.kms().as_ref(),
                        &lesson_cipher,
                    )?,
                    lesson_tokens,
                    evidence_ref,
                    context: None,
                    admission_score,
                    epistemic_kind: clio_types::EpistemicKind::parse(&epistemic_kind)?,
                    created_at,
                },
            ));
        }
        Ok(out)
    })
}

/// SQLite body of `SyncStore::failure_by_attempt` (carrier context joined).
pub(crate) fn failure_by_attempt_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    task_id: &str,
    attempt_n: i64,
) -> Result<Option<FailureRecord>, AmError> {
    use rusqlite::OptionalExtension;
    let raw = store.with_conn(|conn| {
        conn.query_row(
            FAILURE_ATTEMPT_SQL,
            params![bank_id, task_id, attempt_n],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, f64>(10)?,
                    r.get::<_, String>(11)?,
                    r.get::<_, String>(12)?,
                    r.get::<_, Option<String>>(13)?,
                ))
            },
        )
        .optional()
        .am("sync failure by attempt")
    })?;
    raw.map(|f| store.failure_from_raw(f)).transpose()
}
