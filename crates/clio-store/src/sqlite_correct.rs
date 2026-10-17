// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite atomic discrete correction
//! (`correct_item`): one transaction closing the prior assertion and inserting
//! the corrected one with lineage links and `correct` telemetry rows.
//!
//! ## Owns
//! - `correct_item_impl` SQLite SQL (prior close + corrected insert + audit)
//! - The correction-path stored-subject comparison (move refusal)
//!
//! ## Does not own
//! - Reason masking and continuous routing (`clio-compliance`)
//! - Postgres correction SQL (see `postgres_correct`)
//!
//! ## Boundary
//!
//! Changes to SQLite correction SQL should normally require changes here.
//! Changes to the correction contract should not require changes here.
//!
//! Keep this module focused on the SQLite discrete correction.

use crate::audit_model::{CorrectionOutcome, CorrectionSpec};
use crate::item_persist::{
    kind_columns, map_item_insert, require_actor, seal_dual, source_type_sql,
};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode, MemoryItem};
use rusqlite::{OptionalExtension, Transaction, params};

impl SqliteStore {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn correct_item_impl(
        &self,
        spec: &CorrectionSpec,
    ) -> Result<CorrectionOutcome, AmError> {
        if spec.bank_id.is_empty() || spec.prior_item_id.is_empty() || spec.reason.trim().is_empty()
        {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id, prior item id, and a non-empty reason are required",
            ));
        }
        require_actor(&spec.actor)?;
        spec.new_item.validate()?;
        let dek_ref = self.kms().ensure_dek(&spec.new_item.subject_id)?;
        let cipher = seal_dual(
            self.kms().as_ref(),
            &spec.new_item.subject_id,
            &spec.new_item,
        )?;
        let (category, episodic_type) = kind_columns(&spec.new_item);
        let source_type = source_type_sql(&spec.new_item);
        let ts = spec.timestamp.clone();
        let close_detail = audit_detail(
            &spec.new_item,
            &spec.new_item.id,
            &spec.reason,
            "corrected_by",
        );
        let new_detail = audit_detail(
            &spec.new_item,
            &spec.prior_item_id,
            &spec.reason,
            "corrects_item_id",
        );
        self.with_conn(|conn| {
            let tx = conn.transaction().am("correct begin")?;
            let prior: Option<(Option<String>, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT discarded_at, tx_until, subject_id FROM items \
                     WHERE id = ?1 AND bank_id = ?2",
                    params![spec.prior_item_id, spec.bank_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .am("correct prior lookup")?;
            let Some((discarded, tx_until, prior_subject)) = prior else {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("item `{}` not found in bank", spec.prior_item_id),
                ));
            };
            if discarded.is_some() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "item `{}` is discarded and cannot be corrected",
                        spec.prior_item_id
                    ),
                ));
            }
            if tx_until.is_some() {
                return Ok(already_superseded(spec));
            }
            // The subject anchors the DEK and erasure scope: corrections may
            // not re-point an item to another subject. A NULL stored subject
            // also mismatches and is rejected before any mutation.
            if prior_subject.as_deref() != Some(spec.new_item.subject_id.as_str()) {
                return Err(crate::item_persist::subject_immutable_err(
                    &spec.prior_item_id,
                ));
            }
            let n = tx
                .execute(
                    "UPDATE items
                     SET valid_until = ?1, tx_until = ?1, superseded_by_id = ?2, updated_at = ?1
                     WHERE id = ?3 AND bank_id = ?4 AND tx_until IS NULL",
                    params![
                        spec.timestamp,
                        spec.new_item.id,
                        spec.prior_item_id,
                        spec.bank_id
                    ],
                )
                .am("correct prior close")?;
            if n == 0 {
                // Lost a concurrent supersession race; nothing was written.
                return Ok(already_superseded(spec));
            }
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
                    params![spec.new_item.subject_id, dek_ref, spec.new_item.created_at],
                )
                .am("subject_keys upsert")?;
            crate::item_persist::check_subject_reactivation(
                sk_rows as u64,
                &spec.new_item.subject_id,
            )?;
            map_item_insert(
                tx.execute(
                    "INSERT INTO items (
                        id, bank_id, kind, category, episodic_type, epistemic_kind, source_type,
                        confidence, admission_score, source_ref, content_ciphertext, subject_id,
                        valid_from, valid_until, tx_from, tx_until, corrects_item_id,
                        created_at, updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                        ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18, ?19
                     )",
                    params![
                        spec.new_item.id,
                        spec.new_item.bank_id,
                        spec.new_item.kind.as_str(),
                        category,
                        episodic_type,
                        spec.new_item.epistemic_kind.as_str(),
                        source_type,
                        spec.new_item.confidence,
                        spec.new_item.admission_score,
                        spec.new_item.source_ref,
                        cipher,
                        spec.new_item.subject_id,
                        spec.new_item.valid_from,
                        spec.new_item.valid_until,
                        spec.new_item.tx_from,
                        spec.new_item.tx_until,
                        spec.prior_item_id,
                        spec.new_item.created_at,
                        spec.new_item.updated_at,
                    ],
                )
                .map(|c| c as u64)
                .map_err(|e| e.to_string()),
            )?;
            insert_correct_audit(
                &tx,
                &format!("audit-corrected-{}-{ts}", spec.prior_item_id),
                &spec.bank_id,
                &spec.prior_item_id,
                &spec.new_item,
                &spec.actor,
                &close_detail,
                &spec.timestamp,
            )?;
            insert_correct_audit(
                &tx,
                &format!("audit-correct-{}-{ts}", spec.new_item.id),
                &spec.bank_id,
                &spec.new_item.id,
                &spec.new_item,
                &spec.actor,
                &new_detail,
                &spec.timestamp,
            )?;
            crate::sqlite_index::enqueue_index_pending_tx(&tx, &spec.bank_id, &spec.new_item.id)?;
            tx.commit().am("correct commit")?;
            Ok(CorrectionOutcome {
                prior_item_id: spec.prior_item_id.clone(),
                new_item_id: spec.new_item.id.clone(),
                already_superseded: false,
            })
        })
    }
}

fn already_superseded(spec: &CorrectionSpec) -> CorrectionOutcome {
    CorrectionOutcome {
        prior_item_id: spec.prior_item_id.clone(),
        new_item_id: spec.new_item.id.clone(),
        already_superseded: true,
    }
}

/// `correct` audit detail: the reason plus the lineage pointer under `key`,
/// merged with PII-safe context presence/length/hash (never the raw value).
fn audit_detail(item: &MemoryItem, other_id: &str, reason: &str, key: &str) -> String {
    let mut detail = serde_json::json!({ key: other_id, "reason": reason });
    if let (Some(fields), serde_json::Value::Object(stats)) = (
        detail.as_object_mut(),
        crate::item_persist::context_stats(item),
    ) {
        fields.extend(stats);
    }
    detail.to_string()
}

#[allow(clippy::too_many_arguments)]
fn insert_correct_audit(
    tx: &Transaction<'_>,
    audit_id: &str,
    bank_id: &str,
    item_id: &str,
    item: &MemoryItem,
    actor: &str,
    detail: &str,
    timestamp: &str,
) -> Result<(), AmError> {
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES (?1, ?2, 'correct', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            audit_id,
            bank_id,
            item_id,
            item.category.map(|c| c.as_str()),
            item.epistemic_kind.as_str(),
            item.confidence,
            item.admission_score,
            actor,
            detail,
            timestamp,
        ],
    )
    .am("correct audit insert")?;
    Ok(())
}
