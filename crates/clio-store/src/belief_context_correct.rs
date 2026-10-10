// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the authorized belief-context correction on
//! both storage backends: the single mutation path for a stored belief's
//! `context` outside creation, audited atomically.
//!
//! ## Owns
//! - `SqliteStore::correct_belief_context` and its Postgres counterpart: the
//!   transactional row update, its monotonic change stamp, the `context_as_of`
//!   effective-time stamp, and its hash-only audit row
//!
//! ## Does not own
//! - Create/append persistence and their gate (`sqlite_belief`,
//!   `postgres_belief`, `belief_tx`)
//! - The shared sealing, opening, and telemetry helpers (`belief_store`)
//!
//! ## Boundary
//!
//! Changes to what a correction writes should normally require changes here.
//! Changes to belief creation or append semantics should not.
//!
//! Keep this module focused on the audited correction operation.

use crate::belief_store::{
    belief_context_correction_detail, open_belief_context, seal_belief_context,
    validate_belief_context_correction,
};
use crate::migrate::{IntoAmError, now_iso, now_iso8601};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use clio_types::AmError;

impl SqliteStore {
    /// Authorized belief-context correction on SQLite: the only
    /// mutation path for a stored belief's context outside creation; the audit
    /// row carries no confidence because a correction observes nothing.
    /// Inherent method: the per-backend `BeliefStore` impl blocks own the
    /// create/append SQL surface.
    ///
    /// # Errors
    /// Validation, missing-belief, crypto, or storage failures.
    pub fn correct_belief_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        new_context: &str,
        reason: &str,
        actor: &str,
    ) -> Result<(), AmError> {
        use rusqlite::{OptionalExtension, params};
        validate_belief_context_correction(bank_id, belief_id, new_context, reason, actor)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("belief correct begin")?;
            let row: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT subject_id, context_cipher
                     FROM beliefs WHERE id = ?1 AND bank_id = ?2",
                    params![belief_id, bank_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .am("belief correct lookup")?;
            let (subject_id, prior_cipher) =
                row.ok_or_else(|| crate::belief_tx::belief_not_found(belief_id))?;
            let prior_plain = open_belief_context(self.kms().as_ref(), prior_cipher.as_deref());
            // Hash-only prior/new stats: the prior sealed cipher never
            // enters the audit row, so the prior context is unrecoverable.
            let detail =
                belief_context_correction_detail(prior_plain.as_deref(), new_context, reason);
            let new_cipher =
                seal_belief_context(self.kms().as_ref(), &subject_id, Some(new_context))?
                    .expect("sealing a present context yields a cipher");
            // A correction is a change the change feed must see: bump the row's
            // change stamp so peers replicate the corrected context (the ISO
            // stamp keeps the same ordering axis as the confidence-entry times).
            // MAX keeps the bump monotonic — an appended entry with an older
            // `as_of` must never move the stamp (and the feed selection) back.
            let stamp = now_iso8601();
            tx.execute(
                "UPDATE beliefs SET context_cipher = ?1,
                        updated_at = MAX(COALESCE(updated_at, created_at), ?2),
                        context_as_of = ?2
                 WHERE id = ?3 AND bank_id = ?4",
                params![new_cipher, stamp, belief_id, bank_id],
            )
            .am("belief context correction")?;
            let ts = now_iso();
            tx.execute(
                "INSERT INTO audit_events (
                    id, bank_id, operation, item_id, epistemic_kind, confidence,
                    actor, detail_json, created_at
                 ) VALUES (?1, ?2, 'correct', ?3, 'belief', NULL, ?4, ?5, ?6)",
                params![
                    format!("audit-{belief_id}-ctx-{ts}"),
                    bank_id,
                    belief_id,
                    actor,
                    detail,
                    ts
                ],
            )
            .am("belief context audit insert")?;
            tx.commit().am("belief correct commit")?;
            Ok(())
        })
    }
}

impl PostgresStore {
    /// Postgres counterpart of [`SqliteStore::correct_belief_context`].
    ///
    /// # Errors
    /// Validation, missing-belief, crypto, or storage failures.
    pub fn correct_belief_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        new_context: &str,
        reason: &str,
        actor: &str,
    ) -> Result<(), AmError> {
        validate_belief_context_correction(bank_id, belief_id, new_context, reason, actor)?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("belief correct begin")?;
            let row: Option<(String, Option<String>)> = tx
                .query_opt(
                    "SELECT subject_id, context_cipher
                     FROM beliefs WHERE id = $1 AND bank_id = $2",
                    &[&belief_id, &bank_id],
                )
                .am("belief correct lookup")?
                .map(|r| (r.get(0), r.get(1)));
            let (subject_id, prior_cipher) =
                row.ok_or_else(|| crate::belief_tx::belief_not_found(belief_id))?;
            let prior_plain = open_belief_context(self.kms().as_ref(), prior_cipher.as_deref());
            // Hash-only prior/new stats: the prior sealed cipher never
            // enters the audit row, so the prior context is unrecoverable.
            let detail =
                belief_context_correction_detail(prior_plain.as_deref(), new_context, reason);
            let new_cipher =
                seal_belief_context(self.kms().as_ref(), &subject_id, Some(new_context))?
                    .expect("sealing a present context yields a cipher");
            // A correction is a change the change feed must see: bump the row's
            // change stamp so peers replicate the corrected context (the ISO
            // stamp keeps the same ordering axis as the confidence-entry times).
            // GREATEST keeps the bump monotonic — an appended entry with an
            // older `as_of` must never move the stamp (and the feed selection)
            // back.
            let stamp = now_iso8601();
            tx.execute(
                "UPDATE beliefs SET context_cipher = $1,
                        updated_at = GREATEST(COALESCE(updated_at, created_at), $2),
                        context_as_of = $2
                 WHERE id = $3 AND bank_id = $4",
                &[&new_cipher, &stamp, &belief_id, &bank_id],
            )
            .am("belief context correction")?;
            let ts = now_iso();
            tx.execute(
                "INSERT INTO audit_events (
                    id, bank_id, operation, item_id, epistemic_kind, confidence,
                    actor, detail_json, created_at
                 ) VALUES ($1, $2, 'correct', $3, 'belief', NULL, $4, $5, $6)",
                &[
                    &format!("audit-{belief_id}-ctx-{ts}"),
                    &bank_id,
                    &belief_id,
                    &actor,
                    &detail,
                    &ts,
                ],
            )
            .am("belief context audit insert")?;
            tx.commit().am("belief correct commit")?;
            Ok(())
        })
    }
}
