// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres `audit_trail` reconstruction
//! query: lineage traversal plus telemetry, revision, and belief-trajectory
//! assembly from write-path tables only.
//!
//! ## Owns
//! - `audit_trail_impl` Postgres SQL (anchor resolution, lineage, timeline)
//! - Resolution of the persona write handles (`persona-stable:` /
//!   `persona-preference:`) to the bank's persona `audit_events` rows
//!
//! ## Does not own
//! - Reason masking (`clio-compliance`)
//! - SQLite audit SQL (see `sqlite_audit`)
//!
//! ## Boundary
//!
//! Changes to Postgres audit SQL should normally require changes here.
//! Changes to the trail contract should not require changes here.
//!
//! Keep this module focused on the Postgres audit reconstruction.

use crate::audit_model::{AuditTrailEntry, BeliefConfidencePoint, ItemAuditTrail, ItemRevision};
use crate::migrate::IntoAmError;
use std::fmt::Write;

use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode};

impl PostgresStore {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn audit_trail_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<ItemAuditTrail, AmError> {
        if bank_id.is_empty() || item_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id and item_id are required",
            ));
        }
        self.with_client(|client| {
            // Persona writes record synthetic `item_id` handles that are not
            // ledger items and carry no belief, so the anchor path below would
            // report them missing. Resolve the handle shape to that bank's
            // persona write telemetry instead.
            if let Some(handle) = persona_handle(item_id) {
                let events = persona_handle_events(client, bank_id, handle)?;
                if events.is_empty() {
                    return Err(AmError::new(
                        ErrorCode::ItemNotFound,
                        format!("item `{item_id}` not found in bank"),
                    ));
                }
                return Ok(ItemAuditTrail {
                    item_id: item_id.to_owned(),
                    lineage_ids: vec![item_id.to_owned()],
                    revisions: Vec::new(),
                    events,
                    belief_confidence: Vec::new(),
                });
            }
            let anchor = client
                .query_opt(
                    "SELECT corrects_item_id, superseded_by_id FROM items
                     WHERE id = $1 AND bank_id = $2",
                    &[&item_id, &bank_id],
                )
                .am("audit trail anchor lookup")?;
            let is_belief = if anchor.is_none() {
                client
                    .query_one(
                        "SELECT COUNT(*) FROM beliefs WHERE id = $1 AND bank_id = $2",
                        &[&item_id, &bank_id],
                    )
                    .am("audit trail belief lookup")?
                    .get::<_, i64>(0)
                    > 0
            } else {
                false
            };
            if anchor.is_none() && !is_belief {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("item `{item_id}` not found in bank"),
                ));
            }
            let lineage_ids = match anchor {
                Some(row) => {
                    let corrects: Option<String> = row.get(0);
                    let superseded: Option<String> = row.get(1);
                    walk_lineage(client, bank_id, item_id, corrects, superseded)?
                }
                None => vec![item_id.to_owned()],
            };
            let mut trail = ItemAuditTrail {
                item_id: item_id.to_owned(),
                lineage_ids: lineage_ids.clone(),
                revisions: Vec::new(),
                events: Vec::new(),
                belief_confidence: Vec::new(),
            };
            for id in &lineage_ids {
                let revision = client
                    .query_opt(
                        "SELECT id, corrects_item_id, superseded_by_id, valid_from, valid_until,
                                tx_from, tx_until, discarded_at, category, epistemic_kind,
                                confidence::float8, admission_score::float8, source_ref,
                                created_at, updated_at, content_ciphertext
                         FROM items WHERE id = $1 AND bank_id = $2",
                        &[&id, &bank_id],
                    )
                    .am("audit trail revision read")?
                    .map(|r| {
                        let mut revision = map_revision_row(&r);
                        let cipher: Option<String> = r.get(15);
                        revision.context = crate::item_persist::open_context(
                            self.kms().as_ref(),
                            cipher.as_deref(),
                        );
                        revision
                    });
                if let Some(revision) = revision {
                    trail.revisions.push(revision);
                }
            }
            trail
                .revisions
                .sort_by(|a, b| a.tx_from.cmp(&b.tx_from).then(a.item_id.cmp(&b.item_id)));
            let mut sql = String::from(
                "SELECT created_at, operation, item_id, category, epistemic_kind,
                        confidence::float8, admission_score::float8, actor, detail_json
                 FROM audit_events
                 WHERE bank_id = $1 AND item_id IN (",
            );
            for i in 0..lineage_ids.len() {
                if i > 0 {
                    sql.push_str(", ");
                }
                let _ = write!(sql, "${}", i + 2);
            }
            sql.push(')');
            sql.push_str(" ORDER BY created_at ASC, id ASC");
            let mut bind: Vec<String> = vec![bank_id.to_owned()];
            bind.extend(lineage_ids.iter().cloned());
            let refs: Vec<&(dyn postgres::types::ToSql + Sync)> = bind
                .iter()
                .map(|s| s as &(dyn postgres::types::ToSql + Sync))
                .collect();
            for row in client.query(&sql, &refs).am("audit trail events query")? {
                trail.events.push(map_event_row(&row));
            }
            trail.belief_confidence = belief_trajectory(client, bank_id, &lineage_ids)?;
            Ok(trail)
        })
    }
}

/// Persona write-audit handle prefixes. Persona rows are not ledger items, so
/// `audit_trail` resolves these synthetic handles to the persona write
/// telemetry rows for the bank instead of the item/belief anchor path.
const PERSONA_HANDLE_PREFIXES: [&str; 2] = ["persona-stable:", "persona-preference:"];

/// `Some(handle)` when `item_id` is a persona write-audit handle.
fn persona_handle(item_id: &str) -> Option<&str> {
    PERSONA_HANDLE_PREFIXES
        .iter()
        .any(|prefix| item_id.starts_with(prefix))
        .then_some(item_id)
}

/// The persona write telemetry rows for one handle in the bank, ascending.
fn persona_handle_events(
    client: &mut postgres::Client,
    bank_id: &str,
    handle: &str,
) -> Result<Vec<AuditTrailEntry>, AmError> {
    let rows = client
        .query(
            "SELECT created_at, operation, item_id, category, epistemic_kind,
                    confidence::float8, admission_score::float8, actor, detail_json
             FROM audit_events
             WHERE bank_id = $1 AND item_id = $2
             ORDER BY created_at ASC, id ASC",
            &[&bank_id, &handle],
        )
        .am("audit trail persona events query")?;
    Ok(rows.iter().map(map_event_row).collect())
}

/// Map one `audit_events` row (in the shared column order) to a trail entry.
fn map_event_row(row: &postgres::Row) -> AuditTrailEntry {
    let detail_json: String = row.get(8);
    let detail: serde_json::Value =
        serde_json::from_str(&detail_json).unwrap_or(serde_json::Value::Null);
    let reason = detail
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    AuditTrailEntry {
        timestamp: row.get(0),
        operation: row.get(1),
        item_id: row.get(2),
        category: row.get(3),
        epistemic_kind: row.get(4),
        confidence: row.get(5),
        admission_score: row.get(6),
        actor: row.get(7),
        reason,
        detail,
    }
}

fn map_revision_row(row: &postgres::Row) -> ItemRevision {
    ItemRevision {
        item_id: row.get(0),
        corrects_item_id: row.get(1),
        superseded_by_id: row.get(2),
        valid_from: row.get(3),
        valid_until: row.get(4),
        tx_from: row.get(5),
        tx_until: row.get(6),
        discarded_at: row.get(7),
        category: row.get(8),
        epistemic_kind: row.get(9),
        confidence: row.get(10),
        admission_score: row.get(11),
        source_ref: row.get(12),
        context: None,
        created_at: row.get(13),
        updated_at: row.get(14),
    }
}

/// Walk the full lineage both ways with a cycle guard.
fn walk_lineage(
    client: &mut postgres::Client,
    bank_id: &str,
    anchor_id: &str,
    corrects: Option<String>,
    superseded: Option<String>,
) -> Result<Vec<String>, AmError> {
    let mut visited: Vec<String> = vec![anchor_id.to_owned()];
    let mut backwards: Vec<String> = Vec::new();
    let mut cur = corrects;
    while let Some(prev) = cur {
        if visited.contains(&prev) || backwards.contains(&prev) {
            break;
        }
        let row = client
            .query_opt(
                "SELECT corrects_item_id FROM items WHERE id = $1 AND bank_id = $2",
                &[&prev, &bank_id.to_owned()],
            )
            .am("audit trail backward walk")?;
        backwards.push(prev);
        cur = row.and_then(|r| r.get(0));
    }
    backwards.reverse();
    let mut forwards: Vec<String> = Vec::new();
    let mut forward = superseded;
    while let Some(next_id) = forward {
        if visited.contains(&next_id) || forwards.contains(&next_id) {
            break;
        }
        visited.push(next_id.clone());
        forwards.push(next_id.clone());
        forward = client
            .query_opt(
                "SELECT superseded_by_id FROM items WHERE id = $1 AND bank_id = $2",
                &[&next_id, &bank_id.to_owned()],
            )
            .am("audit trail forward walk")?
            .and_then(|r| r.get(0));
    }
    Ok(backwards
        .into_iter()
        .chain(std::iter::once(anchor_id.to_owned()))
        .chain(forwards)
        .collect())
}

/// Append-only belief confidence trajectory for the lineage ids.
fn belief_trajectory(
    client: &mut postgres::Client,
    bank_id: &str,
    lineage_ids: &[String],
) -> Result<Vec<BeliefConfidencePoint>, AmError> {
    let mut sql = String::from(
        "SELECT e.belief_id, e.as_of, e.confidence::float8, e.evidence_ref, e.source_type
         FROM belief_confidence_entries e
         JOIN beliefs b ON b.id = e.belief_id
         WHERE b.bank_id = $1 AND e.belief_id IN (",
    );
    for i in 0..lineage_ids.len() {
        if i > 0 {
            sql.push_str(", ");
        }
        let _ = write!(sql, "${}", i + 2);
    }
    sql.push(')');
    sql.push_str(" ORDER BY e.as_of ASC, e.id ASC");
    let mut bind: Vec<String> = vec![bank_id.to_owned()];
    bind.extend(lineage_ids.iter().cloned());
    let refs: Vec<&(dyn postgres::types::ToSql + Sync)> = bind
        .iter()
        .map(|s| s as &(dyn postgres::types::ToSql + Sync))
        .collect();
    let rows = client.query(&sql, &refs).am("belief trajectory query")?;
    Ok(rows
        .iter()
        .map(|r| BeliefConfidencePoint {
            belief_id: r.get(0),
            as_of: r.get(1),
            confidence: r.get(2),
            evidence_ref: r.get(3),
            source_type: r.get(4),
        })
        .collect())
}

#[cfg(test)]
#[path = "postgres_audit_tests.rs"]
mod postgres_audit_tests;
