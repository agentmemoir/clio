// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for extracted SQLite sync SQL bodies behind the
//! `SyncStore` impl: the per-bank journal append
//! transaction and the watermark change feeds for items, association edges,
//! and triples. The context-bearing persona/belief feeds live in the sibling
//! `sync_records_persona` module and are re-exported here.
//!
//! ## Owns
//! - SQLite SQL bodies for the item/assoc/triple change feeds and journal append
//!
//! ## Does not own
//! - The `SyncStore` impl block (see `sqlite_sync`)
//! - Persona/belief feed SQL (see `sync_records_persona`)
//! - Persona/belief repository SQL (`sqlite_persona`, `sqlite_belief`)
//! - Apply semantics (see `clio-sync`)
//!
//! ## Boundary
//!
//! Changes to SQLite sync feed SQL should normally require changes here.
//! Changes to apply or conflict semantics should not require changes here.
//!
//! Keep this module focused on the SQLite sync feed surface.

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::sync_store_rows::split_push_watermark;
use clio_types::AmError;
use rusqlite::params;

#[path = "sync_records_persona.rs"]
pub(crate) mod persona;

pub(crate) use persona::{
    feed_beliefs_sqlite, feed_persona_prefs_sqlite, feed_persona_stable_sqlite,
};

/// Change-feed SQL: items moved past a watermark (SQLite `?N` placeholders).
/// Task/failure carrier items are excluded: their carrier rows travel with the
/// structured `task`/`failure` mutations (which carry the item AND the record),
/// so a generic item upsert would collide with the carrier insert. `since`
/// binds as a compound `(stamp, entity)` pair: rows resume strictly after
/// `(stamp, entity_id)` in the `ORDER BY stamp, id` order, with the
/// epoch-millis sentinel guard unchanged.
pub(crate) const FEED_ITEMS_SQL: &str = "SELECT id, tx_from, tx_until, discarded_at, \
  COALESCE(updated_at, created_at) AS stamp FROM items WHERE bank_id = ?1 \
  AND COALESCE(episodic_type, '') NOT IN ('task', 'failure') \
  AND (?2 = '1970-01-01T00:00:00Z' OR tx_from > ?2 OR COALESCE(tx_until, '') > ?2 \
  OR COALESCE(discarded_at, '') > ?2 OR COALESCE(updated_at, created_at) > ?2 \
  OR (COALESCE(updated_at, created_at) = ?2 AND id > ?3)) \
  ORDER BY stamp, id LIMIT ?4";

/// Change-feed SQL: association edges updated past a watermark (compound
/// `(last_updated_at, id)` resume).
pub(crate) const FEED_ASSOC_SQL: &str = "SELECT id, last_updated_at FROM assoc_edges \
  WHERE bank_id = ?1 AND (?2 = '1970-01-01T00:00:00Z' OR last_updated_at > ?2 \
  OR (last_updated_at = ?2 AND id > ?3)) \
  ORDER BY last_updated_at, id LIMIT ?4";

/// Change-feed SQL: triples closed on either axis past a watermark. The resume
/// key is the row's close stamp (`COALESCE(tx_until, valid_until, '')`), the
/// same stamp the packaged invalidate mutation carries.
pub(crate) const FEED_TRIPLES_SQL: &str = "SELECT id, COALESCE(tx_until, valid_until, '') \
  FROM triples WHERE bank_id = ?1 \
  AND (?2 = '1970-01-01T00:00:00Z' OR COALESCE(tx_until, '') > ?2 \
  OR COALESCE(valid_until, '') > ?2 \
  OR COALESCE(tx_until, valid_until, '') > ?2 \
  OR (COALESCE(tx_until, valid_until, '') = ?2 AND id > ?3)) \
  ORDER BY COALESCE(tx_until, valid_until, ''), id LIMIT ?4";

/// SQLite body of `SyncStore::feed_items`.
pub(crate) fn feed_items_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<crate::sync_store::SyncFeedRow>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn.prepare(FEED_ITEMS_SQL).am("sync feed items prepare")?;
        let mapped = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    Ok(crate::sqlite_sync_rows::feed_item_row(
                        r.get::<_, String>(0)?.as_str(),
                        r.get::<_, String>(1)?.as_str(),
                        r.get::<_, Option<String>>(2)?.as_deref(),
                        r.get::<_, Option<String>>(3)?.as_deref(),
                        r.get::<_, String>(4)?.as_str(),
                    ))
                },
            )
            .am("sync feed items query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync feed items map")?;
        Ok(mapped)
    })
}

/// SQLite body of `SyncStore::feed_assoc`.
pub(crate) fn feed_assoc_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<crate::sync_store::SyncFeedRow>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn.prepare(FEED_ASSOC_SQL).am("sync feed assoc prepare")?;
        let mapped = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    let id: String = r.get(0)?;
                    let stamp: String = r.get(1)?;
                    Ok(crate::sync_store::SyncFeedRow {
                        entity_kind: "assoc_edge".into(),
                        entity_id: id,
                        op: "upsert".into(),
                        transaction_time: stamp,
                    })
                },
            )
            .am("sync feed assoc query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync feed assoc map")?;
        Ok(mapped)
    })
}

/// SQLite body of `SyncStore::feed_triples`.
pub(crate) fn feed_triples_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<crate::sync_store::SyncFeedRow>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(FEED_TRIPLES_SQL)
            .am("sync feed triples prepare")?;
        let mapped = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    let id: String = r.get(0)?;
                    let close: String = r.get(1)?;
                    Ok(crate::sync_store::SyncFeedRow {
                        entity_kind: "triple".into(),
                        entity_id: id,
                        op: "invalidate".into(),
                        transaction_time: close,
                    })
                },
            )
            .am("sync feed triples query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync feed triples map")?;
        Ok(mapped)
    })
}

/// SQLite body of `SyncStore::journal_append` (idempotent per-bank append).
pub(crate) fn journal_append_sqlite(
    store: &SqliteStore,
    row: &crate::sync_store::SyncJournalRow,
) -> Result<crate::sync_store::JournalAppendOutcome, AmError> {
    use crate::sync_store::JournalAppendOutcome;
    use rusqlite::OptionalExtension;
    store.with_conn(|conn| {
        let tx = conn.transaction().am("sync journal begin")?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT seq FROM sync_mutations WHERE bank_id = ?1 AND sync_mutation_id = ?2",
                params![row.bank_id, row.sync_mutation_id],
                |r| r.get(0),
            )
            .optional()
            .am("sync journal dedupe probe")?;
        if let Some(seq) = existing {
            tx.commit().am("sync journal commit (no-op)")?;
            return Ok(JournalAppendOutcome {
                seq,
                inserted: false,
            });
        }
        let seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM sync_mutations WHERE bank_id = ?1",
                params![row.bank_id],
                |r| r.get(0),
            )
            .am("sync journal next seq")?;
        let encrypted: i64 = i64::from(row.client_encrypted);
        tx.execute(
            "INSERT INTO sync_mutations (bank_id, seq, sync_mutation_id, entity_kind, \
             entity_id, op, transaction_time, device_id, content_hash, content_sealed, \
             encrypted, admission_receipt, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                row.bank_id,
                seq,
                row.sync_mutation_id,
                row.entity_kind,
                row.entity_id,
                row.op,
                row.transaction_time,
                row.device_id,
                row.content_hash,
                row.content_sealed,
                encrypted,
                row.admission_receipt,
                row.created_at,
            ],
        )
        .am("sync journal insert")?;
        tx.commit().am("sync journal commit")?;
        Ok(JournalAppendOutcome {
            seq,
            inserted: true,
        })
    })
}
