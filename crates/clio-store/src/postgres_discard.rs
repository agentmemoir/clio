// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres operations-removal (`discard`)
//! write and identity read.
//!
//! ## Owns
//! - `get_item_identity` / `discard_item` Postgres implementations (single
//!   audited `UPDATE items SET discarded_at, discard_reason` plus the
//!   telemetry row inside the same transaction)
//!
//! ## Does not own
//! - Tool-level validation, confirmation gating, or in-memory event sinks (see `clio-write`)
//! - Content sealing and DEK handling (discard never touches either)
//!
//! ## Boundary
//!
//! Changes to Postgres discard SQL should normally require changes here.
//! Changes to SQLite discard SQL should not require changes here.

use crate::migrate::{IntoAmError, now_iso};
use crate::model::ItemIdentity;
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode};

impl PostgresStore {
    pub(crate) fn get_item_identity_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<ItemIdentity>, AmError> {
        self.with_client(|client| {
            let row = client
                .query_opt(
                    "SELECT category, epistemic_kind FROM items
                     WHERE id = $1 AND bank_id = $2 AND discarded_at IS NULL",
                    &[&item_id, &bank_id],
                )
                .am("item identity get")?;
            Ok(row.map(|r| ItemIdentity {
                item_id: item_id.to_owned(),
                category: r.get(0),
                epistemic_kind: r.get(1),
            }))
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
        let ts = now_iso();
        let detail = serde_json::json!({ "reason": reason }).to_string();
        self.with_client(|client| {
            let mut tx = client.transaction().am("discard begin")?;
            let row = tx
                .query_opt(
                    "SELECT category, epistemic_kind, discarded_at FROM items
                     WHERE id = $1 AND bank_id = $2",
                    &[&item_id, &bank_id],
                )
                .am("discard lookup")?;
            let Some(r) = row else {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("item `{item_id}` not found in bank"),
                ));
            };
            let category: Option<String> = r.get(0);
            let epistemic_kind: String = r.get(1);
            let discarded: Option<String> = r.get(2);
            if discarded.is_some() {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("item `{item_id}` is already discarded"),
                ));
            }
            let n = tx
                .execute(
                    "UPDATE items SET discarded_at = $1, discard_reason = $2, updated_at = $3
                     WHERE id = $4 AND bank_id = $5 AND discarded_at IS NULL",
                    &[&discarded_at, &reason, &ts, &item_id, &bank_id],
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
                 ) VALUES ($1, $2, 'discard', $3, $4, $5, NULL, NULL, $6, $7, $8)",
                &[
                    &format!("audit-discard-{item_id}-{ts}"),
                    &bank_id,
                    &item_id,
                    &category,
                    &epistemic_kind,
                    &actor,
                    &detail,
                    &ts,
                ],
            )
            .am("discard audit insert")?;
            // Operations-removal cascade: distilled units claiming the
            // removed item drop it from their claim set and are marked stale,
            // inside the same transaction. Ids and counts only.
            crate::postgres_distilled_cascade::cascade_source_removed_tx(
                &mut tx,
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
