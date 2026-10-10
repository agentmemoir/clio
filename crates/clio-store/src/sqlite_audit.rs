// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite `audit_trail` reconstruction
//! query: lineage traversal plus telemetry, revision, and belief-trajectory
//! assembly from write-path tables only.
//!
//! ## Owns
//! - `audit_trail_impl` SQLite SQL (anchor resolution, lineage walk, timeline)
//! - Resolution of the persona write handles (`persona-stable:` /
//!   `persona-preference:`) to the bank's persona `audit_events` rows
//!
//! ## Does not own
//! - Reason masking (`clio-compliance`)
//! - Postgres audit SQL (see `postgres_audit`)
//!
//! ## Boundary
//!
//! Changes to SQLite audit SQL should normally require changes here.
//! Changes to the trail contract should not require changes here.
//!
//! Keep this module focused on the SQLite audit reconstruction.

use crate::audit_model::{AuditTrailEntry, BeliefConfidencePoint, ItemAuditTrail, ItemRevision};
use std::fmt::Write;

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode};
use rusqlite::{OptionalExtension, params, params_from_iter};

impl SqliteStore {
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
        self.with_conn(|conn| {
            // Persona writes record synthetic `item_id` handles that are not
            // ledger items and carry no belief, so the anchor path below would
            // report them missing. Resolve the handle shape to that bank's
            // persona write telemetry instead.
            if let Some(handle) = persona_handle(item_id) {
                let events = persona_handle_events(conn, bank_id, handle)?;
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
            let anchor_items: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT corrects_item_id, superseded_by_id FROM items
                     WHERE id = ?1 AND bank_id = ?2",
                    params![item_id, bank_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .am("audit trail anchor lookup")?;
            let is_belief = if anchor_items.is_none() {
                let exists: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM beliefs WHERE id = ?1 AND bank_id = ?2",
                        params![item_id, bank_id],
                        |r| r.get(0),
                    )
                    .am("audit trail belief lookup")?;
                exists > 0
            } else {
                false
            };
            if anchor_items.is_none() && !is_belief {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("item `{item_id}` not found in bank"),
                ));
            }
            let lineage_ids = match anchor_items {
                Some((corrects, superseded)) => {
                    walk_lineage(conn, bank_id, item_id, corrects, superseded)?
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
                let row: Option<(ItemRevision, Option<String>)> = conn
                    .query_row(
                        "SELECT id, corrects_item_id, superseded_by_id, valid_from, valid_until,
                                tx_from, tx_until, discarded_at, category, epistemic_kind,
                                confidence, admission_score, source_ref, created_at, updated_at,
                                content_ciphertext
                         FROM items WHERE id = ?1 AND bank_id = ?2",
                        params![id, bank_id],
                        |r| Ok((map_revision_row(r)?, r.get(15)?)),
                    )
                    .optional()
                    .am("audit trail revision read")?;
                if let Some((mut revision, cipher)) = row {
                    revision.context =
                        crate::item_persist::open_context(self.kms().as_ref(), cipher.as_deref());
                    trail.revisions.push(revision);
                }
            }
            trail
                .revisions
                .sort_by(|a, b| a.tx_from.cmp(&b.tx_from).then(a.item_id.cmp(&b.item_id)));
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT created_at, operation, item_id, category, epistemic_kind,
                            confidence, admission_score, actor, detail_json
                     FROM audit_events
                     WHERE bank_id = ?1 AND item_id IN ({})
                     ORDER BY created_at ASC, id ASC",
                    in_clause(lineage_ids.len())
                ))
                .am("audit trail events prepare")?;
            let mut bind: Vec<String> = vec![bank_id.to_owned()];
            bind.extend(lineage_ids.iter().cloned());
            let rows = stmt
                .query_map(params_from_iter(bind.iter()), map_event_row)
                .am("audit trail events query")?;
            for row in rows {
                trail.events.push(row.am("audit trail event row")?);
            }
            trail.belief_confidence = belief_trajectory(conn, bank_id, &lineage_ids)?;
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
    conn: &rusqlite::Connection,
    bank_id: &str,
    handle: &str,
) -> Result<Vec<AuditTrailEntry>, AmError> {
    let mut stmt = conn
        .prepare(
            "SELECT created_at, operation, item_id, category, epistemic_kind,
                    confidence, admission_score, actor, detail_json
             FROM audit_events
             WHERE bank_id = ?1 AND item_id = ?2
             ORDER BY created_at ASC, id ASC",
        )
        .am("audit trail persona events prepare")?;
    let rows = stmt
        .query_map(params![bank_id, handle], map_event_row)
        .am("audit trail persona events query")?;
    rows.collect::<Result<Vec<_>, _>>()
        .am("audit trail persona events")
}

/// Map one `audit_events` row (in the shared column order) to a trail entry.
fn map_event_row(r: &rusqlite::Row<'_>) -> Result<AuditTrailEntry, rusqlite::Error> {
    let detail_json: String = r.get(8)?;
    let detail: serde_json::Value =
        serde_json::from_str(&detail_json).unwrap_or(serde_json::Value::Null);
    let reason = detail
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(AuditTrailEntry {
        timestamp: r.get(0)?,
        operation: r.get(1)?,
        item_id: r.get(2)?,
        category: r.get(3)?,
        epistemic_kind: r.get(4)?,
        confidence: r.get(5)?,
        admission_score: r.get(6)?,
        actor: r.get(7)?,
        reason,
        detail,
    })
}

fn map_revision_row(r: &rusqlite::Row<'_>) -> Result<ItemRevision, rusqlite::Error> {
    Ok(ItemRevision {
        item_id: r.get(0)?,
        corrects_item_id: r.get(1)?,
        superseded_by_id: r.get(2)?,
        valid_from: r.get(3)?,
        valid_until: r.get(4)?,
        tx_from: r.get(5)?,
        tx_until: r.get(6)?,
        discarded_at: r.get(7)?,
        category: r.get(8)?,
        epistemic_kind: r.get(9)?,
        confidence: r.get(10)?,
        admission_score: r.get(11)?,
        source_ref: r.get(12)?,
        context: None,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
    })
}

/// Walk the full lineage both ways with a cycle guard.
fn walk_lineage(
    conn: &rusqlite::Connection,
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
        let next: Option<Option<String>> = conn
            .query_row(
                "SELECT corrects_item_id FROM items WHERE id = ?1 AND bank_id = ?2",
                params![prev, bank_id],
                |r| r.get(0),
            )
            .optional()
            .am("audit trail backward walk")?;
        backwards.push(prev);
        cur = next.flatten();
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
        forward = conn
            .query_row(
                "SELECT superseded_by_id FROM items WHERE id = ?1 AND bank_id = ?2",
                params![next_id, bank_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .am("audit trail forward walk")?
            .flatten();
    }
    Ok(backwards
        .into_iter()
        .chain(std::iter::once(anchor_id.to_owned()))
        .chain(forwards)
        .collect())
}

/// Append-only belief confidence trajectory for the lineage ids.
fn belief_trajectory(
    conn: &rusqlite::Connection,
    bank_id: &str,
    lineage_ids: &[String],
) -> Result<Vec<BeliefConfidencePoint>, AmError> {
    let mut sql = String::from(
        "SELECT e.belief_id, e.as_of, e.confidence, e.evidence_ref, e.source_type
         FROM belief_confidence_entries e
         JOIN beliefs b ON b.id = e.belief_id
         WHERE b.bank_id = ?1 AND e.belief_id IN (",
    );
    for i in 0..lineage_ids.len() {
        if i > 0 {
            sql.push_str(", ");
        }
        let _ = write!(sql, "?{}", i + 2);
    }
    sql.push(')');
    sql.push_str(" ORDER BY e.as_of ASC, e.id ASC");
    let mut stmt = conn.prepare(&sql).am("belief trajectory prepare")?;
    let mut bind: Vec<String> = vec![bank_id.to_owned()];
    bind.extend(lineage_ids.iter().cloned());
    let rows = stmt
        .query_map(params_from_iter(bind.iter()), |r| {
            Ok(BeliefConfidencePoint {
                belief_id: r.get(0)?,
                as_of: r.get(1)?,
                confidence: r.get(2)?,
                evidence_ref: r.get(3)?,
                source_type: r.get(4)?,
            })
        })
        .am("belief trajectory query")?;
    rows.collect::<Result<Vec<_>, _>>()
        .am("belief trajectory row")
}

/// `?2, ?3, ...` fragment for an IN list of `n` items (first bind is ?1).
fn in_clause(n: usize) -> String {
    (0..n)
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(", ")
}
