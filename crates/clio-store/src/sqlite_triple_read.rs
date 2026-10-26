// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite triple read, end, and item invalidation.
//!
//! ## Owns
//! - `get_triple` / `query_triples` / `triple_end` / `invalidate_item` on SQLite
//! - Surfacing the carrier item's sealed `context` onto triple reads
//!
//! ## Does not own
//! - Atomic supersede writes (see `sqlite_triple`)
//! - Triple-trajectory history reads (see `sqlite_history_triple`)
//!
//! ## Boundary
//!
//! Changes to SQLite triple read/end SQL should normally require changes here.

use crate::item_persist::open_context;
use crate::migrate::{IntoAmError, close_stamp, now_iso};
use crate::sqlite::SqliteStore;
use crate::triple_map::{filter_triples, map_triple_row, query_where_clause};
use clio_types::{AmError, ErrorCode, TripleEndOutcome, TripleQuery, TripleRecord};
use rusqlite::{OptionalExtension, params, params_from_iter};

impl SqliteStore {
    pub(crate) fn get_triple_impl(
        &self,
        bank_id: &str,
        edge_id: &str,
    ) -> Result<Option<TripleRecord>, AmError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT t.id, t.bank_id, t.item_id, t.subject, t.predicate, t.object_ciphertext,
                        t.epistemic_kind, t.source_type, t.confidence, t.valid_from,
                        t.valid_until, t.tx_from, t.tx_until, t.snapshot_ref, t.gist_ref,
                        t.created_at, t.updated_at, i.content_ciphertext
                 FROM triples t LEFT JOIN items i ON i.id = t.item_id
                 WHERE t.id = ?1 AND t.bank_id = ?2",
                params![edge_id, bank_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<f64>>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, String>(11)?,
                        r.get::<_, Option<String>>(12)?,
                        r.get::<_, Option<String>>(13)?,
                        r.get::<_, Option<String>>(14)?,
                        r.get::<_, String>(15)?,
                        r.get::<_, Option<String>>(16)?,
                        r.get::<_, Option<String>>(17)?,
                    ))
                },
            )
            .optional()
            .am("triples get")?
            .map(|t| {
                let mut rec = map_triple_row(
                    t.0,
                    t.1,
                    t.2,
                    t.3,
                    t.4,
                    crate::triple_cipher::open_optional_object(
                        self.kms().as_ref(),
                        t.5.as_deref(),
                    )?,
                    t.6,
                    t.7,
                    t.8,
                    t.9,
                    t.10,
                    t.11,
                    t.12,
                    t.13,
                    t.14,
                    t.15,
                    t.16,
                )?;
                rec.context = open_context(self.kms().as_ref(), t.17.as_deref());
                Ok(rec)
            })
            .transpose()
        })
    }

    pub(crate) fn query_triples_impl(
        &self,
        query: &TripleQuery,
    ) -> Result<Vec<TripleRecord>, AmError> {
        query.validate()?;
        let rows = self.with_conn(|conn| {
            let (where_sql, values) = query_where_clause(query, |i| format!("?{i}"));
            let sql = format!(
                "SELECT t.id, t.bank_id, t.item_id, t.subject, t.predicate, t.object_ciphertext,
                        t.epistemic_kind, t.source_type, t.confidence, t.valid_from,
                        t.valid_until, t.tx_from, t.tx_until, t.snapshot_ref, t.gist_ref,
                        t.created_at, t.updated_at, i.content_ciphertext
                 FROM triples t LEFT JOIN items i ON i.id = t.item_id
                 WHERE {where_sql}"
            );
            let mut stmt = conn.prepare(&sql).am("triples query prepare")?;
            let mapped = stmt
                .query_map(params_from_iter(values), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<f64>>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, String>(11)?,
                        r.get::<_, Option<String>>(12)?,
                        r.get::<_, Option<String>>(13)?,
                        r.get::<_, Option<String>>(14)?,
                        r.get::<_, String>(15)?,
                        r.get::<_, Option<String>>(16)?,
                        r.get::<_, Option<String>>(17)?,
                    ))
                })
                .am("triples query")?;
            let mut out = Vec::new();
            for row in mapped {
                let t = row.am("triples row")?;
                let mut rec = map_triple_row(
                    t.0,
                    t.1,
                    t.2,
                    t.3,
                    t.4,
                    crate::triple_cipher::open_optional_object(
                        self.kms().as_ref(),
                        t.5.as_deref(),
                    )?,
                    t.6,
                    t.7,
                    t.8,
                    t.9,
                    t.10,
                    t.11,
                    t.12,
                    t.13,
                    t.14,
                    t.15,
                    t.16,
                )?;
                rec.context = open_context(self.kms().as_ref(), t.17.as_deref());
                out.push(rec);
            }
            Ok(out)
        })?;
        Ok(filter_triples(rows, query))
    }

    pub(crate) fn list_triples_impl(&self, bank_id: &str) -> Result<Vec<TripleRecord>, AmError> {
        if bank_id.is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT t.id, t.bank_id, t.item_id, t.subject, t.predicate, t.object_ciphertext,
                            t.epistemic_kind, t.source_type, t.confidence, t.valid_from,
                            t.valid_until, t.tx_from, t.tx_until, t.snapshot_ref, t.gist_ref,
                            t.created_at, t.updated_at, i.content_ciphertext
                     FROM triples t LEFT JOIN items i ON i.id = t.item_id
                     WHERE t.bank_id = ?1
                     ORDER BY t.tx_from ASC, t.id ASC",
                )
                .am("triples list prepare")?;
            let mapped = stmt
                .query_map(params![bank_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<f64>>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, Option<String>>(10)?,
                        r.get::<_, String>(11)?,
                        r.get::<_, Option<String>>(12)?,
                        r.get::<_, Option<String>>(13)?,
                        r.get::<_, Option<String>>(14)?,
                        r.get::<_, String>(15)?,
                        r.get::<_, Option<String>>(16)?,
                        r.get::<_, Option<String>>(17)?,
                    ))
                })
                .am("triples list")?;
            let mut out = Vec::new();
            for row in mapped {
                let t = row.am("triples row")?;
                let mut rec = map_triple_row(
                    t.0,
                    t.1,
                    t.2,
                    t.3,
                    t.4,
                    crate::triple_cipher::open_optional_object(
                        self.kms().as_ref(),
                        t.5.as_deref(),
                    )?,
                    t.6,
                    t.7,
                    t.8,
                    t.9,
                    t.10,
                    t.11,
                    t.12,
                    t.13,
                    t.14,
                    t.15,
                    t.16,
                )?;
                rec.context = open_context(self.kms().as_ref(), t.17.as_deref());
                out.push(rec);
            }
            Ok(out)
        })
    }

    pub(crate) fn triple_end_impl(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        object: Option<&str>,
        valid_until: &str,
        tx_until: &str,
    ) -> Result<TripleEndOutcome, AmError> {
        if bank_id.is_empty() || subject.is_empty() || predicate.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id, subject, and predicate are required",
            ));
        }
        if valid_until.is_empty() || tx_until.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "valid_until and tx_until are required",
            ));
        }
        let ts = now_iso();
        self.with_conn(|conn| {
            let tx = conn.transaction().am("triple_end begin")?;
            let mut stmt = tx
                .prepare(
                    "SELECT id, object_ciphertext, tx_from FROM triples
                     WHERE bank_id = ?1 AND subject = ?2 AND predicate = ?3
                       AND tx_until IS NULL",
                )
                .am("triple_end select")?;
            let rows = stmt
                .query_map(params![bank_id, subject, predicate], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .am("triple_end query")?;
            let mut closed = Vec::new();
            for row in rows {
                let (id, cipher, tx_from) = row.am("triple_end row")?;
                // The asserted value is sealed, so an `object` filter
                // is applied here after opening each candidate row's value
                // rather than in SQL.
                if let Some(wanted) = object
                    && crate::triple_cipher::open_optional_object(
                        self.kms().as_ref(),
                        cipher.as_deref(),
                    )? != wanted
                {
                    continue;
                }
                closed.push((id, tx_from));
            }
            drop(stmt);
            if closed.is_empty() {
                tx.commit().am("triple_end noop commit")?;
                return Ok(TripleEndOutcome {
                    closed_ids: Vec::new(),
                    already_ended: true,
                    replacement_id: None,
                });
            }
            for (id, tx_from) in &closed {
                tx.execute(
                    "UPDATE triples
                     SET valid_until = COALESCE(valid_until, ?1), tx_until = ?2, updated_at = ?3
                     WHERE id = ?4 AND bank_id = ?5",
                    params![valid_until, close_stamp(tx_from, tx_until), ts, id, bank_id],
                )
                .am("triple_end close")?;
            }
            tx.commit().am("triple_end commit")?;
            Ok(TripleEndOutcome {
                closed_ids: closed.into_iter().map(|(id, _)| id).collect(),
                already_ended: false,
                replacement_id: None,
            })
        })
    }

    pub(crate) fn invalidate_item_impl(
        &self,
        bank_id: &str,
        item_id: &str,
        valid_until: &str,
        tx_until: &str,
        replacement_id: Option<&str>,
        actor: &str,
    ) -> Result<TripleEndOutcome, AmError> {
        if bank_id.is_empty() || item_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id and item_id are required",
            ));
        }
        if actor.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "actor is required on invalidate writes",
            ));
        }
        if self.get_triple_impl(bank_id, item_id)?.is_some() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "id refers to an SPO triple edge; use triple_end instead of invalidate",
            ));
        }
        self.with_conn(|conn| {
            let tx = conn.transaction().am("invalidate begin")?;
            let outcome = invalidate_item_tx(
                &tx,
                bank_id,
                item_id,
                valid_until,
                tx_until,
                replacement_id,
                actor,
            )?;
            tx.commit().am("invalidate commit")?;
            Ok(outcome)
        })
    }
}

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
    let open: Option<(Option<String>, Option<String>, Option<String>, String)> = tx
        .query_row(
            "SELECT valid_until, tx_until, category, epistemic_kind
             FROM items WHERE id = ?1 AND bank_id = ?2",
            params![item_id, bank_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .am("invalidate item lookup")?;
    let Some((vu, tu, category, epistemic_kind)) = open else {
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
    let n = tx
        .execute(
            "UPDATE items SET valid_until = ?1, tx_until = ?2, superseded_by_id = ?3,
                    updated_at = ?4
             WHERE id = ?5 AND bank_id = ?6",
            params![valid_until, tx_until, replacement_id, ts, item_id, bank_id],
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
#[path = "sqlite_triple_read_tests.rs"]
mod sqlite_triple_read_tests;

#[cfg(test)]
#[path = "sqlite_triple_read_query_tests.rs"]
mod sqlite_triple_read_query_tests;
