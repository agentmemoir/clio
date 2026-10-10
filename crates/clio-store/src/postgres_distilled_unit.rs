// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres storage of the hub-distilled
//! derivation record: insert, bank-scoped read and list, the reconciliation
//! write (prior close + successor insert + record update), and the claim/stale
//! record update with its snapshot re-projection.
//!
//! ## Owns
//! - The `distilled_units` Postgres SQL and row decoding
//! - `revise_distilled_unit` / `update_distilled_unit` Postgres implementations
//!
//! ## Does not own
//! - Record shape/validation (`distilled_unit`) or item creation (`postgres_memory`)
//! - SQLite SQL (see `sqlite_distilled_unit`)
//! - Discard-path cascade (`postgres_distilled_cascade`) and discard validation
//!   and telemetry (`postgres_discard`, `clio-write`)
//!
//! ## Boundary
//!
//! Changes to Postgres record SQL should normally require changes here.
//! Changes to the record shape should not require changes here.
//!
//! Keep this module focused on Postgres derivation-record persistence.

use crate::distilled_evidence::{decode_evidence, encode_evidence};
use crate::distilled_revision::{revision_prior_id, snapshot_projection, validate_revision};
use crate::distilled_unit::{
    DistilledUnitRecord, decode_source_ids, encode_source_ids, record_version_from_i64,
    revision_from_i64,
};
use crate::item_persist::{open_dual, require_actor, seal_dual, seal_dual_content};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode, MemoryItem};
use postgres::Client;

/// Shared column projection for the `distilled_units` table.
const SELECT_COLS: &str = "bank_id, item_id, hub_id, source_ids_json, marker, record_version,
                distilled_at, revision, stale_reason, stale_detail, stale_at, evidence_json";

fn map_row(row: &postgres::Row) -> Result<DistilledUnitRecord, AmError> {
    let source_raw: String = row.get(3);
    // Pre-v3 rows carry the column default '[]' and an empty trail.
    let evidence_raw: String = row.get(11);
    Ok(DistilledUnitRecord {
        bank_id: row.get(0),
        item_id: row.get(1),
        hub_id: row.get(2),
        source_ids: decode_source_ids(&source_raw)?,
        marker: row.get(4),
        record_version: record_version_from_i64(i64::from(row.get::<_, i32>(5)))?,
        distilled_at: row.get(6),
        revision: revision_from_i64(i64::from(row.get::<_, i32>(7)))?,
        stale_reason: row.get(8),
        stale_detail: row.get(9),
        stale_at: row.get(10),
        evidence: decode_evidence(&evidence_raw)?,
    })
}

/// Transaction-scoped record insert; the item row is written by the caller so
/// both land in one transaction.
pub(crate) fn insert_tx(
    tx: &mut postgres::Transaction<'_>,
    record: &DistilledUnitRecord,
) -> Result<(), AmError> {
    let version = i32::try_from(record.record_version).map_err(|_| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit record_version exceeds the Postgres integer range",
        )
    })?;
    let revision = i32::try_from(record.revision).map_err(|_| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit revision exceeds the Postgres integer range",
        )
    })?;
    tx.execute(
        "INSERT INTO distilled_units (
            bank_id, item_id, hub_id, source_ids_json, marker, record_version, distilled_at,
            revision, stale_reason, stale_detail, stale_at, evidence_json
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        &[
            &record.bank_id,
            &record.item_id,
            &record.hub_id,
            &encode_source_ids(&record.source_ids),
            &record.marker,
            &version,
            &record.distilled_at,
            &revision,
            &record.stale_reason,
            &record.stale_detail,
            &record.stale_at,
            &encode_evidence(&record.evidence),
        ],
    )
    .am("distilled unit insert pg")?;
    Ok(())
}

/// A record's version as the Postgres `integer` the column holds.
///
/// The record's own `validate` has already rejected a version above `i32::MAX`
/// with this message, so the narrowing is unreachable in practice; it stays a
/// `Result` so a caller outside that path cannot write a wrapped value.
pub(crate) fn record_version_to_i32(record_version: u32) -> Result<i32, AmError> {
    i32::try_from(record_version).map_err(|_| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit record_version exceeds the Postgres integer range",
        )
    })
}

/// A record's revision as the Postgres `integer` the column holds, for the same
/// reason as [`record_version_to_i32`].
pub(crate) fn revision_to_i32(revision: u32) -> Result<i32, AmError> {
    i32::try_from(revision).map_err(|_| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "distilled unit revision exceeds the Postgres integer range",
        )
    })
}

/// Transaction-scoped record update; keeps the claim set and reconciliation
/// state in step with the item write the same transaction performs.
pub(crate) fn update_record_tx(
    tx: &mut postgres::Transaction<'_>,
    record: &DistilledUnitRecord,
) -> Result<(), AmError> {
    let version = record_version_to_i32(record.record_version)?;
    let revision = revision_to_i32(record.revision)?;
    let n = tx
        .execute(
            "UPDATE distilled_units SET
                source_ids_json = $1, marker = $2, record_version = $3, distilled_at = $4,
                revision = $5, stale_reason = $6, stale_detail = $7, stale_at = $8,
                evidence_json = $9
             WHERE bank_id = $10 AND item_id = $11",
            &[
                &encode_source_ids(&record.source_ids),
                &record.marker,
                &version,
                &record.distilled_at,
                &revision,
                &record.stale_reason,
                &record.stale_detail,
                &record.stale_at,
                &encode_evidence(&record.evidence),
                &record.bank_id,
                &record.item_id,
            ],
        )
        .am("distilled unit record update pg")?;
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
    client: &mut Client,
    bank_id: &str,
    item_id: &str,
) -> Result<Option<DistilledUnitRecord>, AmError> {
    let row = client
        .query_opt(
            &format!(
                "SELECT {SELECT_COLS} FROM distilled_units WHERE bank_id = $1 AND item_id = $2"
            ),
            &[&bank_id, &item_id],
        )
        .am("distilled unit get pg")?;
    row.map(|r| map_row(&r)).transpose()
}

/// List one bank's records ordered by unit id (deterministic). Transaction
/// variant: a `postgres::Transaction` does not deref to the client, so the
/// cascade reads its rows through the open transaction.
pub(crate) fn list_tx(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
) -> Result<Vec<DistilledUnitRecord>, AmError> {
    let rows = tx
        .query(
            &format!(
                "SELECT {SELECT_COLS} FROM distilled_units WHERE bank_id = $1 ORDER BY item_id"
            ),
            &[&bank_id],
        )
        .am("distilled unit list tx pg")?;
    rows.iter().map(map_row).collect()
}

/// List one bank's records ordered by unit id (deterministic).
pub(crate) fn list(
    client: &mut Client,
    bank_id: &str,
) -> Result<Vec<DistilledUnitRecord>, AmError> {
    let rows = client
        .query(
            &format!(
                "SELECT {SELECT_COLS} FROM distilled_units WHERE bank_id = $1 ORDER BY item_id"
            ),
            &[&bank_id],
        )
        .am("distilled unit list pg")?;
    rows.iter().map(map_row).collect()
}

/// The current revision item's ciphertext and stored subject; `None` when the
/// row's content is unreadable for erasure reasons (purged or shredded), which
/// callers must treat as a dead unit and leave untouched.
fn load_current_item_tx(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<Option<(Option<String>, Option<String>, Option<String>)>, AmError> {
    let row = tx
        .query_opt(
            "SELECT content_ciphertext, subject_id, tx_until FROM items WHERE id = $1 AND bank_id = $2",
            &[&item_id, &bank_id],
        )
        .am("distilled unit current item load pg")?;
    Ok(row.map(|r| (r.get(0), r.get(1), r.get(2))))
}

/// Re-project the current revision item's snapshot from `record` and re-seal
/// the content inside the caller's transaction. The gist and context halves
/// are preserved byte for byte; only the snapshot projection moves.
///
/// Returns `Ok(false)` when the row is unreadable — no ciphertext, no
/// subject, a closed row, or a body that will not decrypt: the unit is dead
/// and its record must stay untouched.
pub(crate) fn reproject_snapshot_tx(
    tx: &mut postgres::Transaction<'_>,
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
        "UPDATE items SET content_ciphertext = $1 WHERE id = $2 AND bank_id = $3",
        &[&sealed, &item_id, &record.bank_id],
    )
    .am("distilled unit snapshot reprojection pg")?;
    Ok(true)
}

impl PostgresStore {
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("distilled unit commit begin pg")?;
            crate::postgres_memory::create_memory_item_tx(&mut tx, item, &cipher, &dek_ref, actor)?;
            insert_tx(&mut tx, record)?;
            tx.commit().am("distilled unit commit pg")?;
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
        self.with_client(|client| get(client, bank_id, item_id))
    }

    /// Bank-scoped record list. See [`Store::list_distilled_units`].
    ///
    /// [`Store::list_distilled_units`]: crate::store::Store::list_distilled_units
    pub(crate) fn list_distilled_units_impl(
        &self,
        bank_id: &str,
    ) -> Result<Vec<DistilledUnitRecord>, AmError> {
        self.with_client(|client| list(client, bank_id))
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("distilled revision begin pg")?;
            let n = tx
                .execute(
                    "UPDATE items
                     SET valid_until = $1, tx_until = $1, superseded_by_id = $2, updated_at = $1
                     WHERE id = $3 AND bank_id = $4 AND tx_until IS NULL",
                    &[&ts, &new_item.id, &prior_id, &record.bank_id],
                )
                .am("distilled revision prior close pg")?;
            if n == 0 {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("prior revision `{prior_id}` is already closed or missing"),
                ));
            }
            // The item-create transaction body handles subject keys, the audit
            // row, and the index enqueue; the lineage column points at the
            // closed prior, mirroring the correction insert.
            crate::postgres_memory::create_memory_item_tx(
                &mut tx, new_item, &cipher, &dek_ref, actor,
            )?;
            tx.execute(
                "UPDATE items SET corrects_item_id = $1 WHERE id = $2 AND bank_id = $3",
                &[&prior_id, &new_item.id, &record.bank_id],
            )
            .am("distilled revision lineage link pg")?;
            update_record_tx(&mut tx, record)?;
            tx.commit().am("distilled revision commit pg")?;
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("distilled unit update begin pg")?;
            let projected = reproject_snapshot_tx(&mut tx, self.kms().as_ref(), record)?;
            if projected {
                update_record_tx(&mut tx, record)?;
            }
            tx.commit().am("distilled unit update commit pg")?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "postgres_distilled_unit_tests.rs"]
pub(crate) mod postgres_distilled_unit_tests;

#[cfg(test)]
#[path = "postgres_distilled_unit_cas_tests.rs"]
mod postgres_distilled_unit_cas_tests;
