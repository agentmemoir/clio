// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres history sync SQL bodies
//! behind the `SyncStore` impl: the task/failure change feeds and the
//! apply read/write primitives (`task_current`, `task_put`,
//! `failure_by_attempt`, `failure_put`), materialized through the existing
//! atomic history commit paths.
//!
//! ## Owns
//! - Postgres SQL bodies for the task/failure sync feeds and apply primitives
//!
//! ## Does not own
//! - The `SyncStore for PostgresStore` impl block (`postgres_sync`), which
//!   delegates to these bodies
//! - Item / assoc / triple / persona / belief feed SQL (`postgres_sync_records`)
//! - SQLite history sync persistence (`sqlite_sync_history`)
//!
//! ## Boundary
//!
//! Changes to Postgres task/failure sync SQL should normally require changes
//! here. Changes to apply or conflict semantics should not.
//!
//! Keep this module focused on Postgres history sync persistence.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::postgres_sync_rows::limit_i64;
use crate::sync_store_rows::split_push_watermark;
use clio_types::{AmError, FailureRecord, TaskRecord};

/// Failure columns for the `failure_by_attempt` apply read (mirrors
/// `postgres_history_read`); the carrier context is appended by the query.
const FAILURE_ATTEMPT_COLS: &str = "f.id, f.bank_id, f.item_id, f.task_id, f.subject_id, \
     f.attempt_n, f.what_failed_cipher, f.lesson_cipher, f.lesson_tokens, f.evidence_ref, \
     f.admission_score::float8, f.epistemic_kind, f.created_at";

/// Postgres body of `SyncStore::feed_tasks`: every task version whose change
/// stamp moved past the watermark, opened from its DEK-sealed columns
/// (compound `(created_at, record id)` resume; several versions share one
/// `task_id`, so the record id breaks ties).
pub(crate) fn feed_tasks_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, TaskRecord)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT t.id, t.item_id, t.task_id, t.subject_id, t.version, t.status, \
                 t.definition_cipher, t.steps_cipher, t.evidence_ref, t.admission_score::float8, \
                 t.created_at FROM task_records t WHERE t.bank_id = $1 \
                 AND ($2 = '1970-01-01T00:00:00Z' OR t.created_at > $2 \
                      OR (t.created_at = $2 AND t.id > $3)) \
                 ORDER BY t.created_at, t.id LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed tasks")?;
        let mut out = Vec::new();
        for row in &rows {
            let definition_cipher: String = row.get(6);
            let steps_cipher: String = row.get(7);
            out.push((
                row.get(10),
                TaskRecord {
                    id: row.get(0),
                    bank_id: bank_id.to_owned(),
                    item_id: row.get(1),
                    task_id: row.get(2),
                    subject_id: row.get(3),
                    version: i64::from(row.get::<_, i32>(4)),
                    definition: crate::history_store::open_history_text(
                        store.kms().as_ref(),
                        &definition_cipher,
                    )?,
                    steps: crate::history_store::open_steps(store.kms().as_ref(), &steps_cipher)?,
                    status: row.get(5),
                    evidence_ref: row.get(8),
                    context: None,
                    admission_score: row.get(9),
                    created_at: row.get(10),
                },
            ));
        }
        Ok(out)
    })
}

/// Postgres body of `SyncStore::feed_failures`.
pub(crate) fn feed_failures_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, FailureRecord)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT f.id, f.item_id, f.task_id, f.subject_id, f.attempt_n, \
                 f.what_failed_cipher, f.lesson_cipher, f.lesson_tokens, f.evidence_ref, \
                 f.admission_score::float8, f.epistemic_kind, f.created_at \
                 FROM failure_records f WHERE f.bank_id = $1 \
                 AND ($2 = '1970-01-01T00:00:00Z' OR f.created_at > $2 \
                      OR (f.created_at = $2 AND f.id > $3)) \
                 ORDER BY f.created_at, f.id LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed failures")?;
        let mut out = Vec::new();
        for row in &rows {
            let what_failed_cipher: String = row.get(5);
            let lesson_cipher: String = row.get(6);
            out.push((
                row.get(11),
                FailureRecord {
                    id: row.get(0),
                    bank_id: bank_id.to_owned(),
                    item_id: row.get(1),
                    task_id: row.get(2),
                    subject_id: row.get(3),
                    attempt_n: i64::from(row.get::<_, i32>(4)),
                    what_failed: crate::history_store::open_history_text(
                        store.kms().as_ref(),
                        &what_failed_cipher,
                    )?,
                    lesson: crate::history_store::open_history_text(
                        store.kms().as_ref(),
                        &lesson_cipher,
                    )?,
                    lesson_tokens: i64::from(row.get::<_, i32>(7)),
                    evidence_ref: row.get(8),
                    context: None,
                    admission_score: row.get(9),
                    epistemic_kind: clio_types::EpistemicKind::parse(&row.get::<_, String>(10))?,
                    created_at: row.get(11),
                },
            ));
        }
        Ok(out)
    })
}

/// Postgres body of `SyncStore::failure_by_attempt` (carrier context joined).
pub(crate) fn failure_by_attempt_pg(
    store: &PostgresStore,
    bank_id: &str,
    task_id: &str,
    attempt_n: i64,
) -> Result<Option<FailureRecord>, AmError> {
    // `attempt_n` is an INT4 column: bind the same width back or the driver
    // rejects the parameter (the SQLite body is untyped and needs no cast). An
    // attempt that cannot fit the column cannot match a stored row, so it
    // resolves to the never-matching width instead of a new error path.
    let attempt = i32::try_from(attempt_n).unwrap_or_default();
    store.with_client(|client| {
        let row = client
            .query_opt(
                &format!(
                    "SELECT {FAILURE_ATTEMPT_COLS}, i.content_ciphertext \
                     FROM failure_records f LEFT JOIN items i ON i.id = f.item_id \
                     WHERE f.bank_id = $1 AND f.task_id = $2 AND f.attempt_n = $3 LIMIT 1"
                ),
                &[&bank_id, &task_id, &attempt],
            )
            .am("sync failure by attempt")?;
        row.map(|r| store.failure_from_row(&r)).transpose()
    })
}

#[cfg(test)]
#[path = "postgres_sync_history_tests.rs"]
mod postgres_sync_history_tests;
