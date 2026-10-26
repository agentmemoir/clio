// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the transaction-scoped item invalidation that
//! closes one item row on both axes and writes its audit row.
//!
//! ## Owns
//! - `invalidate_item_tx`: the lookup, the close stamps it binds, and the audit row
//!
//! ## Does not own
//! - The `Store` entry point and triple end paths (see `sqlite_triple_read`)
//! - Batch dispatch over the variants that reach it (see `batch_sqlite`)
//!
//! ## Boundary
//!
//! Changes to the SQLite item invalidate SQL should normally require changes here.

use crate::migrate::{IntoAmError, canonical_stamp, close_stamp, now_iso};
use clio_types::{AmError, ErrorCode, TripleEndOutcome};
use rusqlite::{OptionalExtension, params};

/// Transaction-scoped item invalidate (close both axes + audit row).
pub(crate) fn invalidate_item_tx(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
    valid_until: &str,
    tx_until: &str,
    replacement_id: Option<&str>,
    actor: &str,
) -> Result<TripleEndOutcome, AmError> {
    let ts = now_iso();
    let open: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
    )> = tx
        .query_row(
            "SELECT valid_until, tx_until, category, epistemic_kind, tx_from
             FROM items WHERE id = ?1 AND bank_id = ?2",
            params![item_id, bank_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .am("invalidate item lookup")?;
    let Some((vu, tu, category, epistemic_kind, row_tx_from)) = open else {
        return Err(AmError::new(
            ErrorCode::ItemNotFound,
            format!("item `{item_id}` not found in bank"),
        ));
    };
    if vu.is_some() && tu.is_some() {
        return Ok(TripleEndOutcome {
            closed_ids: vec![item_id.to_owned()],
            already_ended: true,
            replacement_id: replacement_id.map(str::to_owned),
        });
    }
    let tx_until = canonical_stamp(tx_until).unwrap_or_else(|| tx_until.to_owned());
    let tx_close = close_stamp(&row_tx_from, &tx_until);
    let n = tx
        .execute(
            "UPDATE items SET valid_until = ?1, tx_until = ?2, superseded_by_id = ?3,
                    updated_at = ?4
             WHERE id = ?5 AND bank_id = ?6",
            params![valid_until, tx_close, replacement_id, ts, item_id, bank_id],
        )
        .am("invalidate item close")?;
    if n == 0 {
        return Err(AmError::new(
            ErrorCode::Internal,
            "invalidate update matched zero rows",
        ));
    }
    // Telemetry inside the same transaction as the mutation.
    let detail = serde_json::json!({ "replacement_id": replacement_id }).to_string();
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES (?1, ?2, 'invalidate', ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?8)",
        params![
            format!("audit-invalid-{item_id}-{ts}"),
            bank_id,
            item_id,
            category,
            epistemic_kind,
            actor,
            detail,
            ts,
        ],
    )
    .am("invalidate audit insert")?;
    Ok(TripleEndOutcome {
        closed_ids: vec![item_id.to_owned()],
        already_ended: false,
        replacement_id: replacement_id.map(str::to_owned),
    })
}

#[cfg(test)]
#[path = "sqlite_triple_invalidate_tests.rs"]
mod sqlite_triple_invalidate_tests;
