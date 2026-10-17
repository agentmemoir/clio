// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the transaction-scoped opaque item upsert
//! shared by both backends: writing an already-encrypted `ciphertext_backup`
//! content body and its full metadata inventory without re-encryption.
//!
//! ## Owns
//! - `put_opaque_item_sqlite_tx` / `put_opaque_item_postgres_tx` SQL
//!
//! ## Does not own
//! - KMS encrypt/decrypt or DEK custody (`dek`); opaque bodies are stored as-is
//! - DualContent memory-item SQL (see `sqlite_memory` / `postgres_memory`)
//!
//! ## Boundary
//!
//! Changes to how opaque bundle rows are persisted should normally require
//! changes here. Changes to the `BatchWrite` contract should require
//! `batch_model`.
//!
//! Keep this module focused on verbatim opaque row persistence.

use crate::migrate::IntoAmError;
use crate::model::OpaqueItemRecord;
use clio_types::AmError;

/// Upsert one opaque item row on SQLite, preserving metadata and raw ciphertext.
pub(crate) fn put_opaque_item_sqlite_tx(
    tx: &rusqlite::Transaction<'_>,
    item: &OpaqueItemRecord,
) -> Result<(), AmError> {
    use rusqlite::{OptionalExtension, params};
    let shredded: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM subject_keys WHERE subject_id = ?1 AND shredded_at IS NOT NULL",
            params![&item.subject_id],
            |r| r.get(0),
        )
        .optional()
        .am("opaque shredded probe")?;
    if shredded.is_some() {
        return Err(crate::item_persist::shredded_subject_err(&item.subject_id));
    }
    let rows = tx
        .execute(
            "INSERT INTO items (
            id, bank_id, kind, category, episodic_type, epistemic_kind, source_type,
            confidence, admission_score, source_ref, content_ciphertext, subject_id,
            valid_from, valid_until, tx_from, tx_until, created_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7,
            ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17, ?18
         )
         ON CONFLICT(id) DO UPDATE SET
           bank_id = excluded.bank_id,
           kind = excluded.kind,
           category = excluded.category,
           episodic_type = excluded.episodic_type,
           epistemic_kind = excluded.epistemic_kind,
           source_type = excluded.source_type,
           confidence = excluded.confidence,
           admission_score = excluded.admission_score,
           source_ref = excluded.source_ref,
           content_ciphertext = excluded.content_ciphertext,
           subject_id = excluded.subject_id,
           valid_from = excluded.valid_from,
           valid_until = excluded.valid_until,
           tx_from = excluded.tx_from,
           tx_until = excluded.tx_until,
           created_at = excluded.created_at,
           updated_at = excluded.updated_at
         WHERE items.bank_id = excluded.bank_id",
            params![
                item.id,
                item.bank_id,
                item.kind,
                item.category,
                item.episodic_type,
                item.epistemic_kind,
                item.source_type,
                item.confidence,
                item.admission_score,
                item.source_ref,
                item.content_cipher,
                item.subject_id,
                item.valid_from,
                item.valid_until,
                item.tx_from,
                item.tx_until,
                item.created_at,
                item.updated_at,
            ],
        )
        .am("opaque item upsert")?;
    crate::item_persist::check_item_upsert_applied(rows as u64, &item.id)?;
    Ok(())
}

/// Upsert one opaque item row on Postgres, preserving metadata and raw ciphertext.
pub(crate) fn put_opaque_item_postgres_tx(
    tx: &mut postgres::Transaction<'_>,
    item: &OpaqueItemRecord,
) -> Result<(), AmError> {
    let row = tx
        .query_opt(
            "SELECT 1 FROM subject_keys WHERE subject_id = $1 AND shredded_at IS NOT NULL",
            &[&item.subject_id],
        )
        .am("opaque shredded probe")?;
    if row.is_some() {
        return Err(crate::item_persist::shredded_subject_err(&item.subject_id));
    }
    let rows = tx
        .execute(
            "INSERT INTO items (
            id, bank_id, kind, category, episodic_type, epistemic_kind, source_type,
            confidence, admission_score, source_ref, content_ciphertext, subject_id,
            valid_from, valid_until, tx_from, tx_until, created_at, updated_at
         ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8::float8, $9::float8, $10, $11, $12,
            $13, $14, $15, $16, $17, $18
         )
         ON CONFLICT (id) DO UPDATE SET
           bank_id = EXCLUDED.bank_id,
           kind = EXCLUDED.kind,
           category = EXCLUDED.category,
           episodic_type = EXCLUDED.episodic_type,
           epistemic_kind = EXCLUDED.epistemic_kind,
           source_type = EXCLUDED.source_type,
           confidence = EXCLUDED.confidence,
           admission_score = EXCLUDED.admission_score,
           source_ref = EXCLUDED.source_ref,
           content_ciphertext = EXCLUDED.content_ciphertext,
           subject_id = EXCLUDED.subject_id,
           valid_from = EXCLUDED.valid_from,
           valid_until = EXCLUDED.valid_until,
           tx_from = EXCLUDED.tx_from,
           tx_until = EXCLUDED.tx_until,
           created_at = EXCLUDED.created_at,
           updated_at = EXCLUDED.updated_at
         WHERE items.bank_id = EXCLUDED.bank_id",
            &[
                &item.id,
                &item.bank_id,
                &item.kind,
                &item.category,
                &item.episodic_type,
                &item.epistemic_kind,
                &item.source_type,
                &item.confidence,
                &item.admission_score,
                &item.source_ref,
                &item.content_cipher,
                &item.subject_id,
                &item.valid_from,
                &item.valid_until,
                &item.tx_from,
                &item.tx_until,
                &item.created_at,
                &item.updated_at,
            ],
        )
        .am("opaque item upsert")?;
    crate::item_persist::check_item_upsert_applied(rows, &item.id)?;
    Ok(())
}

#[cfg(test)]
#[path = "opaque_item_persist_tests.rs"]
mod opaque_item_persist_tests;
