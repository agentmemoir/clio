// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite storage of the hub-distilled
//! derivation record: insert, bank-scoped read and list, the reconciliation
//! write (prior close + successor insert + record update), the claim/stale
//! record update with its snapshot re-projection, and the operations-removal
//! cascade.
//!
//! ## Owns
//! - The `distilled_units` SQLite SQL and row decoding
//! - `revise_distilled_unit` / `update_distilled_unit` SQLite implementations
//! - The discard-path cascade (`cascade_source_removed_tx`)
//!
//! ## Does not own
//! - Record shape/validation (`distilled_unit`) or item creation (`sqlite_memory`)
//! - Postgres SQL (see `postgres_distilled_unit`)
//! - Discard validation and telemetry (`sqlite_discard`, `clio-write`)
//!
//! ## Boundary
//!
//! Changes to SQLite record SQL should normally require changes here.
//! Changes to the record shape should not require changes here.
//!
//! Keep this module focused on SQLite derivation-record persistence.

use crate::distilled_evidence::{decode_evidence, encode_evidence};
use crate::distilled_revision::{revision_prior_id, snapshot_projection, validate_revision};
use crate::distilled_unit::{
    DistilledUnitRecord, decode_source_ids, encode_source_ids, record_version_from_i64,
    revision_from_i64,
};
use crate::item_persist::{open_dual, require_actor, seal_dual, seal_dual_content};
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::{AmError, ErrorCode, MemoryItem};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::sync::atomic::{AtomicU64, Ordering};

/// Shared column projection for the `distilled_units` table.
const SELECT_COLS: &str = "bank_id, item_id, hub_id, source_ids_json, marker, record_version,
                distilled_at, revision, stale_reason, stale_detail, stale_at, evidence_json";

/// Audit-id disambiguator: two cascade events in one millisecond must not
/// collide on the `audit_events.id` primary key.
static CASCADE_AUDIT_SEQ: AtomicU64 = AtomicU64::new(0);

fn map_row(row: &rusqlite::Row<'_>) -> Result<DistilledUnitRecord, rusqlite::Error> {
    let source_raw: String = row.get(3)?;
    let source_ids = decode_source_ids(&source_raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let version = record_version_from_i64(row.get(5)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Integer, Box::new(e))
    })?;
    // Pre-v2 rows carry the column default 1 and no stale state.
    let revision = revision_from_i64(row.get(7)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Integer, Box::new(e))
    })?;
    // Pre-v3 rows carry the column default '[]' and an empty trail.
    let evidence_raw: String = row.get(11)?;
    let evidence = decode_evidence(&evidence_raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(DistilledUnitRecord {
        bank_id: row.get(0)?,
        item_id: row.get(1)?,
        hub_id: row.get(2)?,
        source_ids,
        marker: row.get(4)?,
        record_version: version,
        distilled_at: row.get(6)?,
        revision,
        stale_reason: row.get(8)?,
        stale_detail: row.get(9)?,
        stale_at: row.get(10)?,
        evidence,
    })
}

/// Transaction-scoped record insert; the item row is written by the caller so
/// both land in one transaction.
pub(crate) fn insert_tx(tx: &Transaction<'_>, record: &DistilledUnitRecord) -> Result<(), AmError> {
    tx.execute(
        "INSERT INTO distilled_units (
            bank_id, item_id, hub_id, source_ids_json, marker, record_version, distilled_at,
            revision, stale_reason, stale_detail, stale_at, evidence_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            record.bank_id,
            record.item_id,
            record.hub_id,
            encode_source_ids(&record.source_ids),
            record.marker,
            i64::from(record.record_version),
            record.distilled_at,
            i64::from(record.revision),
            record.stale_reason,
            record.stale_detail,
            record.stale_at,
            encode_evidence(&record.evidence),
        ],
    )
    .am("distilled unit insert")?;
    Ok(())
}

/// Transaction-scoped record update; keeps the claim set and reconciliation
/// state in step with the item write the same transaction performs.
fn update_record_tx(tx: &Transaction<'_>, record: &DistilledUnitRecord) -> Result<(), AmError> {
    let n = tx
        .execute(
            "UPDATE distilled_units SET
                source_ids_json = ?1, marker = ?2, record_version = ?3, distilled_at = ?4,
                revision = ?5, stale_reason = ?6, stale_detail = ?7, stale_at = ?8,
                evidence_json = ?9
             WHERE bank_id = ?10 AND item_id = ?11",
            params![
                encode_source_ids(&record.source_ids),
                record.marker,
                i64::from(record.record_version),
                record.distilled_at,
                i64::from(record.revision),
                record.stale_reason,
                record.stale_detail,
                record.stale_at,
                encode_evidence(&record.evidence),
                record.bank_id,
                record.item_id,
            ],
        )
        .am("distilled unit record update")?;
    if n == 0 {
        return Err(AmError::new(
            ErrorCode::NotFound,
            format!(
                "distilled unit record `{}` not found in bank",
                record.item_id
            ),
        ));
    }
    Ok(())
}

/// Read one record, bank-scoped.
pub(crate) fn get(
    conn: &Connection,
    bank_id: &str,
    item_id: &str,
) -> Result<Option<DistilledUnitRecord>, AmError> {
    conn.query_row(
        &format!("SELECT {SELECT_COLS} FROM distilled_units WHERE bank_id = ?1 AND item_id = ?2"),
        params![bank_id, item_id],
        map_row,
    )
    .optional()
    .am("distilled unit get")
}

/// List one bank's records ordered by unit id (deterministic).
pub(crate) fn list(bank_id: &str, conn: &Connection) -> Result<Vec<DistilledUnitRecord>, AmError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_COLS} FROM distilled_units WHERE bank_id = ?1 ORDER BY item_id"
        ))
        .am("distilled unit list prepare")?;
    let rows = stmt
        .query_map(params![bank_id], map_row)
        .am("distilled unit list")?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.am("distilled unit list row")?);
    }
    Ok(out)
}

/// The current revision item's ciphertext and stored subject; `None` when the
/// row's content is unreadable for erasure reasons (purged or shredded), which
/// callers must treat as a dead unit and leave untouched.
fn load_current_item_tx(
    tx: &Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<Option<(Option<String>, Option<String>, Option<String>)>, AmError> {
    tx.query_row(
        "SELECT content_ciphertext, subject_id, tx_until FROM items WHERE id = ?1 AND bank_id = ?2",
        params![item_id, bank_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .am("distilled unit current item load")
}

/// Re-project the current revision item's snapshot from `record` and re-seal
/// the content inside the caller's transaction. The gist and context halves
/// are preserved byte for byte; only the snapshot projection moves.
///
/// Returns `Ok(false)` when the row is unreadable — no ciphertext, no
/// subject, a closed row, or a body that will not decrypt: the unit is dead
/// and its record must stay untouched.
pub(crate) fn reproject_snapshot_tx(
    tx: &Transaction<'_>,
    kms: &dyn crate::dek::DekProvider,
    record: &DistilledUnitRecord,
) -> Result<bool, AmError> {
    let item_id = record.current_revision_item_id();
    let Some((cipher, subject, tx_until)) = load_current_item_tx(tx, &record.bank_id, &item_id)?
    else {
        return Ok(false);
    };
    let Some(cipher) = cipher.filter(|c| !c.is_empty()) else {
        return Ok(false);
    };
    let Some(subject) = subject else {
        return Ok(false);
    };
    if tx_until.is_some() {
        // The current revision row was closed outside reconciliation (an
        // operator correction superseded it): historical row, never rewritten.
        return Ok(false);
    }
    // An unreadable body is a dead unit, exactly like a purged or shredded
    // row: the operator is discarding an unrelated item, and one damaged
    // derived row must not reject that discard. `is_unreadable_content` is the
    // workspace predicate covering a malformed envelope, a shredded subject,
    // and a missing or mismatched key. Any other failure — a backend that is
    // down, say — still propagates and rolls the discard back.
    let dual = match open_dual(kms, &cipher) {
        Ok(dual) => dual,
        Err(err) if crate::content_read_errors::is_unreadable_content(&err) => return Ok(false),
        Err(err) => return Err(err),
    };
    let sealed = seal_dual_content(
        kms,
        &subject,
        Some(snapshot_projection(record)),
        dual.gist,
        dual.context,
    )?;
    tx.execute(
        "UPDATE items SET content_ciphertext = ?1 WHERE id = ?2 AND bank_id = ?3",
        params![sealed, item_id, record.bank_id],
    )
    .am("distilled unit snapshot reprojection")?;
    Ok(true)
}

impl SqliteStore {
    /// Atomic item + derivation-record create. See [`Store::commit_distilled_unit`].
    ///
    /// [`Store::commit_distilled_unit`]: crate::store::Store::commit_distilled_unit
    pub(crate) fn commit_distilled_unit_impl(
        &self,
        item: &MemoryItem,
        record: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<(), AmError> {
        item.validate()?;
        record.validate_for_item(item)?;
        require_actor(actor)?;
        let dek_ref = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("distilled unit commit begin")?;
            crate::sqlite_memory::create_memory_item_tx(&tx, item, &cipher, &dek_ref, actor)?;
            insert_tx(&tx, record)?;
            tx.commit().am("distilled unit commit")?;
            Ok(())
        })
    }

    /// Bank-scoped record read. See [`Store::get_distilled_unit`].
    ///
    /// [`Store::get_distilled_unit`]: crate::store::Store::get_distilled_unit
    pub(crate) fn get_distilled_unit_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<DistilledUnitRecord>, AmError> {
        self.with_conn(|conn| get(conn, bank_id, item_id))
    }

    /// Bank-scoped record list. See [`Store::list_distilled_units`].
    ///
    /// [`Store::list_distilled_units`]: crate::store::Store::list_distilled_units
    pub(crate) fn list_distilled_units_impl(
        &self,
        bank_id: &str,
    ) -> Result<Vec<DistilledUnitRecord>, AmError> {
        self.with_conn(|conn| list(bank_id, conn))
    }

    /// Revision write. See [`Store::revise_distilled_unit`].
    ///
    /// [`Store::revise_distilled_unit`]: crate::store::Store::revise_distilled_unit
    pub(crate) fn revise_distilled_unit_impl(
        &self,
        record: &DistilledUnitRecord,
        new_item: &MemoryItem,
        actor: &str,
    ) -> Result<(), AmError> {
        validate_revision(record, new_item)?;
        require_actor(actor)?;
        let dek_ref = self.kms().ensure_dek(&new_item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &new_item.subject_id, new_item)?;
        let ts = record.distilled_at.clone();
        let prior_id = revision_prior_id(record)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("distilled revision begin")?;
            let n = tx
                .execute(
                    "UPDATE items
                     SET valid_until = ?1, tx_until = ?1, superseded_by_id = ?2, updated_at = ?1
                     WHERE id = ?3 AND bank_id = ?4 AND tx_until IS NULL",
                    params![ts, new_item.id, prior_id, record.bank_id],
                )
                .am("distilled revision prior close")?;
            if n == 0 {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("prior revision `{prior_id}` is already closed or missing"),
                ));
            }
            // The item-create transaction body handles subject keys, the audit
            // row, and the index enqueue; the lineage column points at the
            // closed prior, mirroring the correction insert.
            crate::sqlite_memory::create_memory_item_tx(&tx, new_item, &cipher, &dek_ref, actor)?;
            tx.execute(
                "UPDATE items SET corrects_item_id = ?1 WHERE id = ?2 AND bank_id = ?3",
                params![prior_id, new_item.id, record.bank_id],
            )
            .am("distilled revision lineage link")?;
            update_record_tx(&tx, record)?;
            tx.commit().am("distilled revision commit")?;
            Ok(())
        })
    }

    /// Claim/stale update + snapshot re-projection. See
    /// [`Store::update_distilled_unit`].
    ///
    /// [`Store::update_distilled_unit`]: crate::store::Store::update_distilled_unit
    pub(crate) fn update_distilled_unit_impl(
        &self,
        record: &DistilledUnitRecord,
        actor: &str,
    ) -> Result<(), AmError> {
        record.validate()?;
        require_actor(actor)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("distilled unit update begin")?;
            let projected = reproject_snapshot_tx(&tx, self.kms().as_ref(), record)?;
            if projected {
                update_record_tx(&tx, record)?;
            }
            tx.commit().am("distilled unit update commit")?;
            Ok(())
        })
    }
}

/// Operations-removal cascade, inside the discard transaction: every unit in
/// the bank whose claim set contains the removed item drops that id from its
/// claim set and is marked stale with reason `source_removed`, and its current
/// revision item's snapshot is re-projected from the same record state.
///
/// A unit whose current content is unreadable for erasure reasons is left
/// untouched (dead unit; erasure evidence). A discarded unit id that is the
/// record's hub leaves the source ids alone: the hub column is the unit's
/// identity, the unit stays stale, and reconciliation of a discarded hub never
/// rebuilds. Telemetry is ids and counts only.
pub(crate) fn cascade_source_removed_tx(
    tx: &Transaction<'_>,
    kms: &dyn crate::dek::DekProvider,
    bank_id: &str,
    removed_id: &str,
    discarded_at: &str,
    actor: &str,
) -> Result<(), AmError> {
    for mut record in list(bank_id, tx)? {
        if !record.claim_ids().contains(removed_id) {
            continue;
        }
        let claim_before = record.claim_ids().len();
        record.source_ids.retain(|id| id != removed_id);
        // The current revision's evidence list follows the claim set: a source
        // that left active use stops counting toward the proof count. Earlier
        // revisions keep their entries as history of what was claimed then.
        record.drop_source_from_current_evidence(removed_id);
        record.stale_reason = Some(crate::distilled_unit::STALE_SOURCE_REMOVED.to_owned());
        record.stale_at = Some(discarded_at.to_owned());
        record.stale_detail = None;
        if !reproject_snapshot_tx(tx, kms, &record)? {
            continue;
        }
        update_record_tx(tx, &record)?;
        let seq = CASCADE_AUDIT_SEQ.fetch_add(1, Ordering::Relaxed);
        tx.execute(
            "INSERT INTO audit_events (
                id, bank_id, operation, item_id, category, epistemic_kind,
                confidence, admission_score, actor, detail_json, created_at
             ) VALUES (?1, ?2, 'hub_distill_source_removed', ?3, NULL, NULL, NULL, NULL, ?4, ?5, ?6)",
            params![
                format!("audit-hubdistill-cascade-{}-{discarded_at}-{seq}", record.item_id),
                bank_id,
                record.item_id,
                actor,
                serde_json::json!({
                    "removed_id": removed_id,
                    "claim_before": claim_before,
                    "claim_after": record.claim_ids().len(),
                })
                .to_string(),
                discarded_at,
            ],
        )
        .am("distilled cascade audit insert")?;
    }
    Ok(())
}
