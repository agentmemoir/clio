// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite operations-removal (`discard`)
//! write and identity read.
//!
//! ## Owns
//! - `get_item_identity` / `discard_item` SQLite implementations (single audited
//!   `UPDATE items SET discarded_at, discard_reason`)
//!
//! ## Does not own
//! - Tool-level validation, confirmation gating, or in-memory event sinks (see `clio-write`)
//! - Content sealing and DEK handling (discard never touches either)
//!
//! ## Boundary
//!
//! Changes to SQLite discard SQL should normally require changes here.
//! Changes to Postgres discard SQL should not require changes here.

use crate::migrate::IntoAmError;
use crate::model::ItemIdentity;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::{OptionalExtension, params};

impl SqliteStore {
    pub(crate) fn get_item_identity_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<ItemIdentity>, AmError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT category, epistemic_kind FROM items
                 WHERE id = ?1 AND bank_id = ?2 AND discarded_at IS NULL",
                params![item_id, bank_id],
                |r| {
                    Ok(ItemIdentity {
                        item_id: item_id.to_owned(),
                        category: r.get(0)?,
                        epistemic_kind: r.get(1)?,
                    })
                },
            )
            .optional()
            .am("item identity get")
        })
    }

    pub(crate) fn discard_item_impl(
        &self,
        bank_id: &str,
        item_id: &str,
        reason: &str,
        discarded_at: &str,
        actor: &str,
    ) -> Result<ItemIdentity, AmError> {
        if bank_id.is_empty() || item_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id and item_id are required",
            ));
        }
        if reason.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "discard reason is required and must be non-empty",
            ));
        }
        if actor.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "actor is required on discard writes",
            ));
        }
        // The canonical form, because `updated_at` and `discarded_at` are both
        // compared as text by the items change feed and the inspect filter.
        let ts = crate::migrate::now_iso8601();
        let detail = serde_json::json!({ "reason": reason }).to_string();
        self.with_conn(|conn| {
            let tx = conn.transaction().am("discard begin")?;
            let row: Option<(Option<String>, String, Option<String>)> = tx
                .query_row(
                    "SELECT category, epistemic_kind, discarded_at FROM items
                     WHERE id = ?1 AND bank_id = ?2",
                    params![item_id, bank_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .am("discard lookup")?;
            let Some((category, epistemic_kind, discarded)) = row else {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("item `{item_id}` not found in bank"),
                ));
            };
            if discarded.is_some() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("item `{item_id}` is already discarded"),
                ));
            }
            let n = tx
                .execute(
                    "UPDATE items SET discarded_at = ?1, discard_reason = ?2, updated_at = ?3
                     WHERE id = ?4 AND bank_id = ?5 AND discarded_at IS NULL",
                    params![discarded_at, reason, ts, item_id, bank_id],
                )
                .am("discard update")?;
            if n == 0 {
                // Items are never hard-deleted, so a miss on the
                // `discarded_at IS NULL` guard after the live pre-check can
                // only mean a concurrent discard won the race. Surface the
                // documented repeat-discard rejection instead of an internal
                // error.
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("item `{item_id}` is already discarded"),
                ));
            }
            // Telemetry inside the same transaction as the mutation.
            tx.execute(
                "INSERT INTO audit_events (
                    id, bank_id, operation, item_id, category, epistemic_kind,
                    confidence, admission_score, actor, detail_json, created_at
                 ) VALUES (?1, ?2, 'discard', ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?8)",
                params![
                    format!("audit-discard-{item_id}-{ts}"),
                    bank_id,
                    item_id,
                    category,
                    epistemic_kind,
                    actor,
                    detail,
                    ts,
                ],
            )
            .am("discard audit insert")?;
            // Operations-removal cascade: distilled units claiming the
            // removed item drop it from their claim set and are marked stale,
            // inside the same transaction. Ids and counts only.
            crate::sqlite_distilled_unit::cascade_source_removed_tx(
                &tx,
                self.kms().as_ref(),
                bank_id,
                item_id,
                discarded_at,
                actor,
            )?;
            tx.commit().am("discard commit")?;
            Ok(ItemIdentity {
                item_id: item_id.to_owned(),
                category,
                epistemic_kind,
            })
        })
    }
}

#[cfg(test)]
#[path = "sqlite_discard_tests.rs"]
mod sqlite_discard_tests;
