// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite history reads and the SQLite
//! `HistoryStore` implementation.
//!
//! ## Owns
//! - Task/failure/triple-history SELECT SQL and row decryption
//! - The `impl HistoryStore for SqliteStore` delegation surface
//!
//! ## Does not own
//! - History writes (see `sqlite_history_write`) or the trait contract
//!
//! ## Boundary
//!
//! Changes to SQLite history read SQL should normally require changes here.
//! Changes to history writes should not require changes here.

use crate::history_store::{HistoryStore, open_history_text, open_steps};
use crate::item_persist::open_context;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, FailureRecord, MemoryItem, TaskRecord, TimeAxis, TripleRecord};
use rusqlite::{OptionalExtension, params};

type TaskRaw = (
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    Option<String>,
    f64,
    String,
    Option<String>,
);

type FailureRaw = (
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    i64,
    Option<String>,
    f64,
    String,
    String,
    Option<String>,
);

pub(crate) const TASK_COLS: &str = "t.id, t.bank_id, t.item_id, t.task_id, t.subject_id, t.version, t.status, \
     t.definition_cipher, t.steps_cipher, t.evidence_ref, t.admission_score, t.created_at";
pub(crate) const FAILURE_COLS: &str = "f.id, f.bank_id, f.item_id, f.task_id, f.subject_id, f.attempt_n, \
     f.what_failed_cipher, f.lesson_cipher, f.lesson_tokens, f.evidence_ref, f.admission_score, \
     f.epistemic_kind, f.created_at";
/// The sealed item `context` lives inside the carrier's dual-content cipher,
/// so task/failure reads join the backing carrier item (left join: a missing
/// or purged carrier must keep the record readable, with no context).
const TASK_FROM: &str = "FROM task_records t LEFT JOIN items i ON i.id = t.item_id";
const FAILURE_FROM: &str = "FROM failure_records f LEFT JOIN items i ON i.id = f.item_id";
fn map_task_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRaw> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
    ))
}

fn map_failure_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FailureRaw> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
    ))
}

impl SqliteStore {
    pub(crate) fn task_from_raw(&self, t: TaskRaw) -> Result<TaskRecord, AmError> {
        Ok(TaskRecord {
            definition: open_history_text(self.kms().as_ref(), &t.7)?,
            steps: open_steps(self.kms().as_ref(), &t.8)?,
            id: t.0,
            bank_id: t.1,
            item_id: t.2,
            task_id: t.3,
            subject_id: t.4,
            version: t.5,
            status: t.6,
            evidence_ref: t.9,
            admission_score: t.10,
            created_at: t.11,
            context: open_context(self.kms().as_ref(), t.12.as_deref()),
        })
    }

    pub(crate) fn failure_from_raw(&self, f: FailureRaw) -> Result<FailureRecord, AmError> {
        Ok(FailureRecord {
            id: f.0,
            bank_id: f.1,
            item_id: f.2,
            task_id: f.3,
            subject_id: f.4,
            attempt_n: f.5,
            what_failed: open_history_text(self.kms().as_ref(), &f.6)?,
            lesson: open_history_text(self.kms().as_ref(), &f.7)?,
            lesson_tokens: f.8,
            evidence_ref: f.9,
            admission_score: f.10,
            epistemic_kind: clio_types::EpistemicKind::parse(&f.11)?,
            created_at: f.12,
            context: open_context(self.kms().as_ref(), f.13.as_deref()),
        })
    }

    pub(crate) fn get_task_impl(
        &self,
        bank_id: &str,
        task_id: &str,
    ) -> Result<Option<TaskRecord>, AmError> {
        let raw = self.with_conn(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {TASK_COLS}, i.content_ciphertext {TASK_FROM}
                     WHERE t.bank_id = ?1 AND t.task_id = ?2 ORDER BY t.version DESC LIMIT 1"
                ),
                params![bank_id, task_id],
                map_task_row,
            )
            .optional()
            .am("task get")
        })?;
        raw.map(|r| self.task_from_raw(r)).transpose()
    }

    pub(crate) fn task_history_impl(
        &self,
        bank_id: &str,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskRecord>, AmError> {
        self.list_task_rows(
            &format!(
                "SELECT {TASK_COLS}, i.content_ciphertext {TASK_FROM}
                 WHERE t.bank_id = ?1 AND t.task_id = ?2 ORDER BY t.version DESC LIMIT ?3"
            ),
            params![bank_id, task_id, limit as i64],
        )
    }

    pub(crate) fn list_tasks_impl(
        &self,
        bank_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskRecord>, AmError> {
        self.list_task_rows(
            &format!(
                "SELECT {TASK_COLS}, i.content_ciphertext {TASK_FROM}
                 JOIN (SELECT bank_id, task_id, MAX(version) AS mv FROM task_records
                       WHERE bank_id = ?1 GROUP BY bank_id, task_id) m
                   ON t.bank_id = m.bank_id AND t.task_id = m.task_id AND t.version = m.mv
                 ORDER BY t.created_at DESC, t.task_id LIMIT ?2"
            ),
            params![bank_id, limit as i64],
        )
    }

    fn list_task_rows(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<TaskRecord>, AmError> {
        let raw = self.with_conn(|conn| {
            let mut stmt = conn.prepare(sql).am("task list prepare")?;
            let rows = stmt.query_map(params, map_task_row).am("task list query")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("task list row")?);
            }
            Ok(out)
        })?;
        raw.into_iter().map(|r| self.task_from_raw(r)).collect()
    }

    pub(crate) fn get_failure_impl(
        &self,
        bank_id: &str,
        failure_id: &str,
    ) -> Result<Option<FailureRecord>, AmError> {
        let raw = self.with_conn(|conn| {
            conn.query_row(
                &format!(
                    "SELECT {FAILURE_COLS}, i.content_ciphertext {FAILURE_FROM}
                     WHERE f.id = ?1 AND f.bank_id = ?2"
                ),
                params![failure_id, bank_id],
                map_failure_row,
            )
            .optional()
            .am("failure get")
        })?;
        raw.map(|r| self.failure_from_raw(r)).transpose()
    }

    pub(crate) fn failures_for_task_exact_impl(
        &self,
        bank_id: &str,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<FailureRecord>, AmError> {
        self.list_failure_rows(
            &format!(
                "SELECT {FAILURE_COLS}, i.content_ciphertext {FAILURE_FROM}
                 WHERE f.bank_id = ?1 AND f.task_id = ?2 ORDER BY f.attempt_n DESC LIMIT ?3"
            ),
            params![bank_id, task_id, limit as i64],
        )
    }

    pub(crate) fn list_failures_impl(
        &self,
        bank_id: &str,
        limit: usize,
    ) -> Result<Vec<FailureRecord>, AmError> {
        self.list_failure_rows(
            &format!(
                "SELECT {FAILURE_COLS}, i.content_ciphertext {FAILURE_FROM}
                 WHERE f.bank_id = ?1 ORDER BY f.created_at DESC, f.id LIMIT ?2"
            ),
            params![bank_id, limit as i64],
        )
    }

    fn list_failure_rows(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<FailureRecord>, AmError> {
        let raw = self.with_conn(|conn| {
            let mut stmt = conn.prepare(sql).am("failure list prepare")?;
            let rows = stmt
                .query_map(params, map_failure_row)
                .am("failure list query")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("failure list row")?);
            }
            Ok(out)
        })?;
        raw.into_iter().map(|r| self.failure_from_raw(r)).collect()
    }
}

impl HistoryStore for SqliteStore {
    fn commit_task_upsert(
        &self,
        item: &MemoryItem,
        draft: &TaskRecord,
        actor: &str,
    ) -> Result<TaskRecord, AmError> {
        self.commit_task_upsert_impl(item, draft, actor)
    }

    fn get_task(&self, bank_id: &str, task_id: &str) -> Result<Option<TaskRecord>, AmError> {
        self.get_task_impl(bank_id, task_id)
    }

    fn task_history(
        &self,
        bank_id: &str,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskRecord>, AmError> {
        self.task_history_impl(bank_id, task_id, limit)
    }

    fn list_tasks(&self, bank_id: &str, limit: usize) -> Result<Vec<TaskRecord>, AmError> {
        self.list_tasks_impl(bank_id, limit)
    }

    fn commit_failure_record(
        &self,
        item: &MemoryItem,
        draft: &FailureRecord,
        actor: &str,
    ) -> Result<FailureRecord, AmError> {
        self.commit_failure_record_impl(item, draft, actor)
    }

    fn get_failure(
        &self,
        bank_id: &str,
        failure_id: &str,
    ) -> Result<Option<FailureRecord>, AmError> {
        self.get_failure_impl(bank_id, failure_id)
    }

    fn failures_for_task_exact(
        &self,
        bank_id: &str,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<FailureRecord>, AmError> {
        self.failures_for_task_exact_impl(bank_id, task_id, limit)
    }

    fn list_failures(&self, bank_id: &str, limit: usize) -> Result<Vec<FailureRecord>, AmError> {
        self.list_failures_impl(bank_id, limit)
    }

    fn triple_history(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError> {
        self.triple_history_impl(bank_id, subject, predicate, limit)
    }

    fn triple_history_at(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        as_of: &str,
        axis: TimeAxis,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError> {
        self.triple_history_at_impl(bank_id, subject, predicate, as_of, axis, limit)
    }
}
