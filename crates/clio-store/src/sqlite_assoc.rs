// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite association-edge persistence
//! (upsert/get/delete/list), the atomic co-activation reinforce upsert, and
//! PII-safe assoc audit writes.
//!
//! ## Owns
//! - Assoc-edge SQLite SQL bodies behind the `Store` delegators in `sqlite_store`
//!
//! ## Does not own
//! - Envelope/vector/triple SQL (other `sqlite_*` modules) or the weight-policy
//!   math (`assoc`)
//!
//! ## Boundary
//!
//! Changes to SQLite assoc-edge SQL should normally require changes here.
//! Changes to the association formula should not require changes here.

use crate::assoc::{canonical_pair, coactivation_id};
use crate::migrate::{IntoAmError, now_iso};
use crate::model::{AssocEdgeRecord, AssocKind};
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

type EdgeRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    f64,
    String,
);

const SELECT_EDGE: &str = "SELECT id, bank_id, source_id, target_id, assoc_kind, relationship, \
                            weight, last_updated_at FROM assoc_edges";

fn read_edge_row(row: &Row<'_>) -> rusqlite::Result<EdgeRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn parse_edge_row(row: EdgeRow) -> Result<AssocEdgeRecord, AmError> {
    let (id, bank_id, source_id, target_id, kind, relationship, weight, last_updated_at) = row;
    Ok(AssocEdgeRecord {
        id,
        bank_id,
        source_id,
        target_id,
        assoc_kind: AssocKind::parse(&kind)?,
        relationship,
        weight,
        last_updated_at,
    })
}

fn require_pair(bank_id: &str, a: &str, b: &str, eta: f64) -> Result<(), AmError> {
    if bank_id.is_empty() || a.is_empty() || b.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "assoc reinforce requires bank and both item ids",
        ));
    }
    if a == b {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "assoc reinforce requires two distinct item ids",
        ));
    }
    if !(0.0..=1.0).contains(&eta) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "assoc reinforce eta must be in [0, 1]",
        ));
    }
    Ok(())
}

/// Transaction-scoped association-edge upsert (batch executor reuse).
pub(crate) fn put_assoc_edge_tx(
    tx: &rusqlite::Transaction<'_>,
    edge: &AssocEdgeRecord,
) -> Result<(), AmError> {
    let ts = now_iso();
    tx.execute(
        "INSERT INTO assoc_edges (
            id, bank_id, source_id, target_id, assoc_kind, relationship,
            weight, last_updated_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
         ON CONFLICT(id) DO UPDATE SET
           source_id = excluded.source_id,
           target_id = excluded.target_id,
           assoc_kind = excluded.assoc_kind,
           relationship = excluded.relationship,
           weight = excluded.weight,
           last_updated_at = excluded.last_updated_at,
           updated_at = excluded.updated_at
         WHERE assoc_edges.bank_id = excluded.bank_id",
        params![
            edge.id,
            edge.bank_id,
            edge.source_id,
            edge.target_id,
            edge.assoc_kind.as_str(),
            edge.relationship,
            edge.weight,
            edge.last_updated_at,
            ts,
        ],
    )
    .am("assoc_edges upsert")?;
    Ok(())
}

/// Transaction-scoped generic audit-event append (validate telemetry,
/// batch reuse). PII-safe: callers pass ids/counts, never raw payloads.
pub(crate) fn record_audit_event_tx(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    operation: &str,
    item_id: Option<&str>,
    actor: &str,
    detail: &Value,
) -> Result<(), AmError> {
    if bank_id.is_empty() || operation.is_empty() || actor.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "audit event requires bank, operation, and actor",
        ));
    }
    let ts = now_iso();
    let id = format!("audit-{operation}-{ts}-{}", rand::random::<u32>());
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, actor, detail_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            bank_id,
            operation,
            item_id,
            actor,
            detail.to_string(),
            ts
        ],
    )
    .am("audit event insert")?;
    Ok(())
}

impl SqliteStore {
    /// Trait body: insert-or-replace an association edge.
    pub(crate) fn put_assoc_edge_impl(&self, edge: &AssocEdgeRecord) -> Result<(), AmError> {
        if edge.id.is_empty() || edge.bank_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "assoc edge id and bank_id are required",
            ));
        }
        if !(0.0..=1.0).contains(&edge.weight) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "assoc edge weight must be in 0.0..=1.0",
            ));
        }
        self.with_conn(|conn| {
            let tx = conn.transaction().am("assoc edge begin")?;
            put_assoc_edge_tx(&tx, edge)?;
            tx.commit().am("assoc edge commit")?;
            Ok(())
        })
    }

    /// Trait body: read one association edge by id.
    pub(crate) fn get_assoc_edge_impl(
        &self,
        bank_id: &str,
        edge_id: &str,
    ) -> Result<Option<AssocEdgeRecord>, AmError> {
        let row: Option<EdgeRow> = self.with_conn(|conn| {
            conn.query_row(
                &format!("{SELECT_EDGE} WHERE id = ?1 AND bank_id = ?2"),
                params![edge_id, bank_id],
                read_edge_row,
            )
            .optional()
            .am("assoc_edges get")
        })?;
        row.map(parse_edge_row).transpose()
    }

    /// Trait body: delete one association edge by id.
    pub(crate) fn delete_assoc_edge_impl(
        &self,
        bank_id: &str,
        edge_id: &str,
    ) -> Result<bool, AmError> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM assoc_edges WHERE id = ?1 AND bank_id = ?2",
                    params![edge_id, bank_id],
                )
                .am("assoc_edges delete")?;
            Ok(n > 0)
        })
    }

    /// Trait body: list association edges incident to `item_id` (or whole bank).
    pub(crate) fn list_assoc_edges_impl(
        &self,
        bank_id: &str,
        item_id: Option<&str>,
    ) -> Result<Vec<AssocEdgeRecord>, AmError> {
        let rows: Vec<EdgeRow> = self.with_conn(|conn| {
            let mut out = Vec::new();
            if let Some(item) = item_id {
                let sql = format!(
                    "{SELECT_EDGE} WHERE bank_id = ?1 AND (source_id = ?2 OR target_id = ?2) \
                     ORDER BY id"
                );
                let mut stmt = conn.prepare(&sql).am("assoc_edges list prepare")?;
                let mapped = stmt
                    .query_map(params![bank_id, item], read_edge_row)
                    .am("assoc_edges list query")?;
                for row in mapped {
                    out.push(row.expect("assoc edge row"));
                }
            } else {
                let sql = format!("{SELECT_EDGE} WHERE bank_id = ?1 ORDER BY id");
                let mut stmt = conn.prepare(&sql).am("assoc_edges list prepare")?;
                let mapped = stmt
                    .query_map(params![bank_id], read_edge_row)
                    .am("assoc_edges list query")?;
                for row in mapped {
                    out.push(row.expect("assoc edge row"));
                }
            }
            Ok(out)
        })?;
        rows.into_iter().map(parse_edge_row).collect()
    }

    /// Trait body: atomic saturating reinforce of an unordered coactivation pair.
    pub(crate) fn reinforce_assoc_edge_impl(
        &self,
        bank_id: &str,
        a: &str,
        b: &str,
        now_millis: i64,
        eta: f64,
    ) -> Result<AssocEdgeRecord, AmError> {
        require_pair(bank_id, a, b, eta)?;
        let (lo, hi) = canonical_pair(a, b);
        let id = coactivation_id(bank_id, a, b);
        let now = now_millis.to_string();
        let ts = now_iso();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO assoc_edges (
                    id, bank_id, source_id, target_id, assoc_kind, relationship,
                    weight, last_updated_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 'coactivation', NULL, ?5, ?6, ?7, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                   weight = min(1.0, assoc_edges.weight + ?5 * (1.0 - assoc_edges.weight)),
                   last_updated_at = excluded.last_updated_at,
                   updated_at = excluded.updated_at
                 WHERE assoc_edges.bank_id = excluded.bank_id",
                params![id, bank_id, lo, hi, eta, now, ts],
            )
            .am("assoc_edges reinforce")?;
            Ok(())
        })?;
        self.get_assoc_edge_impl(bank_id, &id)?.ok_or_else(|| {
            AmError::new(
                ErrorCode::Internal,
                "reinforced assoc edge vanished after upsert",
            )
        })
    }

    /// Trait body: append a PII-safe association audit event.
    pub(crate) fn record_assoc_audit_impl(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError> {
        if bank_id.is_empty() || operation.is_empty() || actor.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "assoc audit requires bank, operation, and actor",
            ));
        }
        let ts = now_iso();
        let id = format!("assoc-audit-{operation}-{ts}-{}", rand::random::<u32>());
        let detail = detail.to_string();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO audit_events (
                    id, bank_id, operation, item_id, actor, detail_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, bank_id, operation, item_id, actor, detail, ts],
            )
            .am("assoc audit insert")?;
            Ok(())
        })
    }

    /// Trait body: append a PII-safe generic audit event (workspace tools).
    pub(crate) fn record_audit_event_impl(
        &self,
        bank_id: &str,
        operation: &str,
        item_id: Option<&str>,
        actor: &str,
        detail: &Value,
    ) -> Result<(), AmError> {
        self.with_conn(|conn| {
            let tx = conn.transaction().am("audit event begin")?;
            record_audit_event_tx(&tx, bank_id, operation, item_id, actor, detail)?;
            tx.commit().am("audit event commit")?;
            Ok(())
        })
    }
}
