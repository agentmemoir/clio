// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres history reads and the
//! Postgres `HistoryStore` implementation.
//!
//! ## Owns
//! - Task/failure/triple-history SELECT SQL and row decryption
//! - The `impl HistoryStore for PostgresStore` delegation surface
//!
//! ## Does not own
//! - History writes (see `postgres_history_write`) or the trait contract
//!
//! ## Boundary
//!
//! Changes to Postgres history read SQL should normally require changes here.
//! Changes to history writes should not require changes here.

use crate::history_store::{HistoryStore, open_history_text, open_steps};
use crate::item_persist::open_context;
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::triple_map::map_triple_row;
use clio_types::{AmError, FailureRecord, MemoryItem, TaskRecord, TimeAxis, TripleRecord};
use postgres::Row;

const TASK_COLS: &str = "t.id, t.bank_id, t.item_id, t.task_id, t.subject_id, t.version, t.status, \
     t.definition_cipher, t.steps_cipher, t.evidence_ref, t.admission_score::float8, t.created_at";
const FAILURE_COLS: &str = "f.id, f.bank_id, f.item_id, f.task_id, f.subject_id, f.attempt_n, \
     f.what_failed_cipher, f.lesson_cipher, f.lesson_tokens, f.evidence_ref, f.admission_score::float8, \
     f.epistemic_kind, f.created_at";
/// The sealed item `context` lives inside the carrier's dual-content cipher,
/// so task/failure reads join the backing carrier item (left join: a missing
/// or purged carrier must keep the record readable, with no context).
const TASK_FROM: &str = "FROM task_records t LEFT JOIN items i ON i.id = t.item_id";
const FAILURE_FROM: &str = "FROM failure_records f LEFT JOIN items i ON i.id = f.item_id";
const TRIPLE_COLS: &str = "id, bank_id, item_id, subject, predicate, object_ciphertext, \
     epistemic_kind, source_type, confidence::float8, valid_from, valid_until, tx_from, tx_until, \
     snapshot_ref, gist_ref, created_at, updated_at";

impl PostgresStore {
    /// Attach the carrier item's sealed descriptive `context` to each record.
    ///
    /// `None` covers an absent body, no stored context, or an unreadable body
    /// (for example a crypto-shredded subject) so the history read stays
    /// available after erasure.
    fn attach_triple_carrier_context(&self, records: &mut [TripleRecord]) -> Result<(), AmError> {
        for record in records {
            let cipher: Option<String> = self
                .with_client(|client| {
                    client
                        .query_opt(
                            "SELECT content_ciphertext FROM items WHERE id = $1",
                            &[&record.item_id],
                        )
                        .am("triple history carrier context")
                        .map(|row| row.map(|r| r.get::<_, Option<String>>(0)))
                })?
                .flatten();
            record.context = open_context(self.kms().as_ref(), cipher.as_deref());
        }
        Ok(())
    }

    /// Open one history row's sealed asserted value and build the
    /// record. The trajectory query is keyed on `subject` + `predicate`, which
    /// stay plaintext, so only the value needs the DEK.
    fn triple_from_row(&self, r: &Row) -> Result<TripleRecord, AmError> {
        map_triple_row(
            r.get(0),
            r.get(1),
            r.get(2),
            r.get(3),
            r.get(4),
            crate::triple_cipher::open_optional_object(
                self.kms().as_ref(),
                r.get::<_, Option<String>>(5).as_deref(),
            )?,
            r.get(6),
            r.get(7),
            r.get(8),
            r.get(9),
            r.get(10),
            r.get(11),
            r.get(12),
            r.get(13),
            r.get(14),
            r.get(15),
            r.get(16),
        )
    }

    pub(crate) fn task_from_row(&self, t: &Row) -> Result<TaskRecord, AmError> {
        Ok(TaskRecord {
            id: t.get(0),
            bank_id: t.get(1),
            item_id: t.get(2),
            task_id: t.get(3),
            subject_id: t.get(4),
            version: i64::from(t.get::<_, i32>(5)),
            status: t.get(6),
            definition: open_history_text(self.kms().as_ref(), &t.get::<_, String>(7))?,
            steps: open_steps(self.kms().as_ref(), &t.get::<_, String>(8))?,
            evidence_ref: t.get(9),
            admission_score: t.get(10),
            created_at: t.get(11),
            context: open_context(
                self.kms().as_ref(),
                t.get::<_, Option<String>>(12).as_deref(),
            ),
        })
    }

    pub(crate) fn failure_from_row(&self, f: &Row) -> Result<FailureRecord, AmError> {
        Ok(FailureRecord {
            id: f.get(0),
            bank_id: f.get(1),
            item_id: f.get(2),
            task_id: f.get(3),
            subject_id: f.get(4),
            attempt_n: i64::from(f.get::<_, i32>(5)),
            what_failed: open_history_text(self.kms().as_ref(), &f.get::<_, String>(6))?,
            lesson: open_history_text(self.kms().as_ref(), &f.get::<_, String>(7))?,
            lesson_tokens: i64::from(f.get::<_, i32>(8)),
            evidence_ref: f.get(9),
            admission_score: f.get(10),
            epistemic_kind: clio_types::EpistemicKind::parse(&f.get::<_, String>(11))?,
            created_at: f.get(12),
            context: open_context(
                self.kms().as_ref(),
                f.get::<_, Option<String>>(13).as_deref(),
            ),
        })
    }

    pub(crate) fn get_task_impl(
        &self,
        bank_id: &str,
        task_id: &str,
    ) -> Result<Option<TaskRecord>, AmError> {
        let row = self.with_client(|client| {
            client
                .query_opt(
                    &format!(
                        "SELECT {TASK_COLS}, i.content_ciphertext {TASK_FROM}
                         WHERE t.bank_id = $1 AND t.task_id = $2 ORDER BY t.version DESC LIMIT 1"
                    ),
                    &[&bank_id, &task_id],
                )
                .am("task get")
        })?;
        row.as_ref().map(|r| self.task_from_row(r)).transpose()
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
                 WHERE t.bank_id = $1 AND t.task_id = $2 ORDER BY t.version DESC LIMIT $3"
            ),
            &[&bank_id, &task_id, &(limit as i64)],
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
                       WHERE bank_id = $1 GROUP BY bank_id, task_id) m
                   ON t.bank_id = m.bank_id AND t.task_id = m.task_id AND t.version = m.mv
                 ORDER BY t.created_at DESC, t.task_id LIMIT $2"
            ),
            &[&bank_id, &(limit as i64)],
        )
    }

    fn list_task_rows(
        &self,
        sql: &str,
        params: &[&(dyn postgres::types::ToSql + Sync)],
    ) -> Result<Vec<TaskRecord>, AmError> {
        let rows = self.with_client(|client| client.query(sql, params).am("task list query"))?;
        rows.iter().map(|r| self.task_from_row(r)).collect()
    }

    pub(crate) fn get_failure_impl(
        &self,
        bank_id: &str,
        failure_id: &str,
    ) -> Result<Option<FailureRecord>, AmError> {
        let row = self.with_client(|client| {
            client
                .query_opt(
                    &format!(
                        "SELECT {FAILURE_COLS}, i.content_ciphertext {FAILURE_FROM}
                         WHERE f.id = $1 AND f.bank_id = $2"
                    ),
                    &[&failure_id, &bank_id],
                )
                .am("failure get")
        })?;
        row.as_ref().map(|r| self.failure_from_row(r)).transpose()
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
                 WHERE f.bank_id = $1 AND f.task_id = $2 ORDER BY f.attempt_n DESC LIMIT $3"
            ),
            &[&bank_id, &task_id, &(limit as i64)],
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
                 WHERE f.bank_id = $1 ORDER BY f.created_at DESC, f.id LIMIT $2"
            ),
            &[&bank_id, &(limit as i64)],
        )
    }

    fn list_failure_rows(
        &self,
        sql: &str,
        params: &[&(dyn postgres::types::ToSql + Sync)],
    ) -> Result<Vec<FailureRecord>, AmError> {
        let rows = self.with_client(|client| client.query(sql, params).am("failure list query"))?;
        rows.iter().map(|r| self.failure_from_row(r)).collect()
    }

    pub(crate) fn triple_history_impl(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError> {
        let rows = self.with_client(|client| {
            client
                .query(
                    &format!(
                        "SELECT {TRIPLE_COLS} FROM triples
                         WHERE bank_id = $1 AND subject = $2 AND predicate = $3
                         ORDER BY tx_from, id LIMIT $4"
                    ),
                    &[&bank_id, &subject, &predicate, &(limit as i64)],
                )
                .am("triple history query")
        })?;
        rows.iter()
            .map(|r| self.triple_from_row(r))
            .collect::<Result<Vec<_>, _>>()
            .and_then(|mut records| {
                self.attach_triple_carrier_context(&mut records)?;
                Ok(records)
            })
    }

    pub(crate) fn triple_history_at_impl(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        as_of: &str,
        axis: TimeAxis,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError> {
        let axis_col = match axis {
            TimeAxis::Valid => "valid_from",
            TimeAxis::Transaction => "tx_from",
        };
        let rows = self.with_client(|client| {
            // Newest `limit` versions at/under `as_of`, then oldest-first.
            // The axis predicate runs in the inner query so LIMIT cannot drop
            // the version current at `as_of`.
            client
                .query(
                    &format!(
                        "SELECT {TRIPLE_COLS} FROM (
                             SELECT {TRIPLE_COLS} FROM triples
                             WHERE bank_id = $1 AND subject = $2 AND predicate = $3
                               AND {axis_col} <= $4
                             ORDER BY {axis_col} DESC, id DESC LIMIT $5
                         ) AS visible ORDER BY {axis_col} ASC, id ASC"
                    ),
                    &[&bank_id, &subject, &predicate, &as_of, &(limit as i64)],
                )
                .am("triple history at query")
        })?;
        rows.iter()
            .map(|r| self.triple_from_row(r))
            .collect::<Result<Vec<_>, _>>()
            .and_then(|mut records| {
                self.attach_triple_carrier_context(&mut records)?;
                Ok(records)
            })
    }
}

impl HistoryStore for PostgresStore {
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
