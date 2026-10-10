// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres association-edge persistence
//! (upsert/get/delete/list), the atomic co-activation reinforce upsert, and
//! PII-safe assoc audit writes.
//!
//! ## Owns
//! - Assoc-edge Postgres SQL bodies behind the `Store` delegators in `postgres_store`
//!
//! ## Does not own
//! - Envelope/vector/triple SQL (other `postgres_*` modules) or the weight-policy
//!   math (`assoc`)
//!
//! ## Boundary
//!
//! Changes to Postgres assoc-edge SQL should normally require changes here.
//! Changes to the association formula should not require changes here.

use crate::assoc::{canonical_pair, coactivation_id};
use crate::migrate::{IntoAmError, now_iso};
use crate::model::{AssocEdgeRecord, AssocKind};
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode};
use serde_json::Value;

const SELECT_EDGE: &str = "SELECT id, bank_id, source_id, target_id, assoc_kind, relationship, \
                            weight::float8, last_updated_at FROM assoc_edges";

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

fn parse_row(row: &postgres::Row) -> Result<AssocEdgeRecord, AmError> {
    let kind: String = row.get(4);
    Ok(AssocEdgeRecord {
        id: row.get(0),
        bank_id: row.get(1),
        source_id: row.get(2),
        target_id: row.get(3),
        assoc_kind: AssocKind::parse(&kind)?,
        relationship: row.get(5),
        weight: row.get(6),
        last_updated_at: row.get(7),
    })
}

impl PostgresStore {
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("assoc edge begin")?;
            put_assoc_edge_tx(&mut tx, edge)?;
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
        let row = self.with_client(|client| {
            client
                .query_opt(
                    &format!("{SELECT_EDGE} WHERE id = $1 AND bank_id = $2"),
                    &[&edge_id, &bank_id],
                )
                .am("assoc_edges get")
        })?;
        row.as_ref().map(parse_row).transpose()
    }

    /// Trait body: delete one association edge by id.
    pub(crate) fn delete_assoc_edge_impl(
        &self,
        bank_id: &str,
        edge_id: &str,
    ) -> Result<bool, AmError> {
        self.with_client(|client| {
            let n = client
                .execute(
                    "DELETE FROM assoc_edges WHERE id = $1 AND bank_id = $2",
                    &[&edge_id, &bank_id],
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
        let rows = self.with_client(|client| match item_id {
            Some(item) => client
                .query(
                    &format!(
                        "{SELECT_EDGE} WHERE bank_id = $1 AND (source_id = $2 OR target_id = $2) \
                         ORDER BY id"
                    ),
                    &[&bank_id, &item],
                )
                .am("assoc_edges list query"),
            None => client
                .query(
                    &format!("{SELECT_EDGE} WHERE bank_id = $1 ORDER BY id"),
                    &[&bank_id],
                )
                .am("assoc_edges list query"),
        })?;
        rows.iter().map(parse_row).collect()
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
        self.with_client(|client| {
            client
                .execute(
                    "INSERT INTO assoc_edges (
                        id, bank_id, source_id, target_id, assoc_kind, relationship,
                        weight, last_updated_at, created_at, updated_at
                     ) VALUES ($1, $2, $3, $4, 'coactivation', NULL, $5::float8, $6, $7, $7)
                     ON CONFLICT (id) DO UPDATE SET
                       weight = LEAST(1.0, assoc_edges.weight + $5::float8 * (1.0 - assoc_edges.weight)),
                       last_updated_at = EXCLUDED.last_updated_at,
                       updated_at = EXCLUDED.updated_at
                     WHERE assoc_edges.bank_id = EXCLUDED.bank_id",
                    &[&id, &bank_id, &lo, &hi, &eta, &now, &ts],
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
        self.with_client(|client| {
            client
                .execute(
                    "INSERT INTO audit_events (
                        id, bank_id, operation, item_id, actor, detail_json, created_at
                     ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    &[&id, &bank_id, &operation, &item_id, &actor, &detail, &ts],
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("audit event begin")?;
            record_audit_event_tx(&mut tx, bank_id, operation, item_id, actor, detail)?;
            tx.commit().am("audit event commit")?;
            Ok(())
        })
    }
}

/// Transaction-scoped association-edge upsert (batch executor reuse).
pub(crate) fn put_assoc_edge_tx(
    tx: &mut postgres::Transaction<'_>,
    edge: &AssocEdgeRecord,
) -> Result<(), AmError> {
    let ts = now_iso();
    let kind = edge.assoc_kind.as_str().to_owned();
    let weight = edge.weight;
    let rel = edge.relationship.clone();
    tx.execute(
        "INSERT INTO assoc_edges (
            id, bank_id, source_id, target_id, assoc_kind, relationship,
            weight, last_updated_at, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7::float8, $8, $9, $9)
         ON CONFLICT (id) DO UPDATE SET
           source_id = EXCLUDED.source_id,
           target_id = EXCLUDED.target_id,
           assoc_kind = EXCLUDED.assoc_kind,
           relationship = EXCLUDED.relationship,
           weight = EXCLUDED.weight,
           last_updated_at = EXCLUDED.last_updated_at,
           updated_at = EXCLUDED.updated_at
         WHERE assoc_edges.bank_id = EXCLUDED.bank_id",
        &[
            &edge.id,
            &edge.bank_id,
            &edge.source_id,
            &edge.target_id,
            &kind,
            &rel,
            &weight,
            &edge.last_updated_at,
            &ts,
        ],
    )
    .am("assoc_edges upsert")?;
    Ok(())
}

/// Transaction-scoped generic audit-event append (validate telemetry, batch
/// reuse). PII-safe: callers pass ids/counts, never raw payloads.
pub(crate) fn record_audit_event_tx(
    tx: &mut postgres::Transaction<'_>,
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
    let detail = detail.to_string();
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, actor, detail_json, created_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        &[&id, &bank_id, &operation, &item_id, &actor, &detail, &ts],
    )
    .am("audit event insert")?;
    Ok(())
}

#[cfg(test)]
#[path = "postgres_assoc_edge_tests.rs"]
mod postgres_assoc_edge_tests;
