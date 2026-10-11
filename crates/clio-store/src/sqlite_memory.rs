// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for SQLite ungated memory-item repository primitives.
//!
//! ## Owns
//! - SQLite create/update/get for `MemoryItem` and snapshot/gist getters
//!
//! ## Does not own
//! - Domain validation (`clio-types`)
//! - DualContent seal helpers (`item_persist`)
//!
//! ## Boundary
//!
//! Changes to SQLite item SQL should normally require changes here.
//! Changes to Postgres item SQL should not require changes here.

use crate::item_persist::{
    gist_missing, kind_columns, map_item_insert, missing_ciphertext, open_dual, require_actor,
    seal_dual, snapshot_missing, source_type_sql,
};
use crate::migrate::{IntoAmError, now_iso};
use crate::sqlite::SqliteStore;
use clio_types::{
    AmError, EpisodicType, EpistemicKind, ItemKind, MemoryItem, SemanticCategory, SourceType,
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;

/// Shared items column list (id … updated_at).
const ITEM_COLUMNS_SELECT: &str = "SELECT id, bank_id, subject_id, kind, category, episodic_type,
        epistemic_kind, source_type, confidence, admission_score, source_ref, content_ciphertext,
        valid_from, valid_until, tx_from, tx_until, created_at, updated_at FROM items";

/// Id-keyed item lookup with the shared column projection.
const ITEM_COLUMNS_QUERY: &str = "SELECT id, bank_id, subject_id, kind, category, episodic_type,
        epistemic_kind, source_type, confidence, admission_score, source_ref, content_ciphertext,
        valid_from, valid_until, tx_from, tx_until, created_at, updated_at
 FROM items WHERE id = ?1 AND bank_id = ?2";

/// Column tuple type shared by the item getters.
type ItemRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<f64>,
    Option<f64>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
);

/// Read the shared column projection from one row.
fn map_item_columns(r: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
        r.get(15)?,
        r.get(16)?,
        r.get(17)?,
    ))
}

/// Decrypt and type an item row; `None` passes through (missing / wrong bank).
fn map_item_row(
    kms: &dyn crate::dek::DekProvider,
    row: Option<ItemRow>,
) -> Result<Option<MemoryItem>, AmError> {
    let Some((
        id,
        bank_id,
        subject_id,
        kind,
        category,
        episodic_type,
        epistemic_kind,
        source_type,
        confidence,
        admission_score,
        source_ref,
        cipher,
        valid_from,
        valid_until,
        tx_from,
        tx_until,
        created_at,
        updated_at,
    )) = row
    else {
        return Ok(None);
    };
    let cipher = cipher
        .filter(|s| !s.is_empty())
        .ok_or_else(missing_ciphertext)?;
    let dual = open_dual(kms, &cipher)?;
    Ok(Some(MemoryItem {
        id,
        bank_id,
        subject_id,
        kind: ItemKind::parse(&kind)?,
        category: category.map(|c| SemanticCategory::parse(&c)).transpose()?,
        episodic_type: episodic_type.map(|t| EpisodicType::parse(&t)).transpose()?,
        epistemic_kind: EpistemicKind::parse(&epistemic_kind)?,
        source_type: source_type.map(|s| SourceType::parse(&s)).transpose()?,
        confidence,
        admission_score,
        source_ref,
        context: dual.context,
        snapshot: dual.snapshot,
        gist: dual.gist,
        valid_from,
        valid_until,
        tx_from,
        tx_until,
        created_at,
        updated_at,
    }))
}

/// Transaction-scoped item create: subject_keys upsert + items insert +
/// audit row + index-pending enqueue. Shared by the single-op path and the
/// atomic batch executor (`batch_sqlite`).
pub(crate) fn create_memory_item_tx(
    tx: &Transaction<'_>,
    item: &MemoryItem,
    cipher: &str,
    dek_ref: &str,
    actor: &str,
) -> Result<(), AmError> {
    let (category, episodic_type) = kind_columns(item);
    let source_type = source_type_sql(item);
    let ts = now_iso();
    tx.execute(
        "INSERT INTO banks (id, created_at, updated_at) VALUES (?1, ?2, ?2)
         ON CONFLICT (id) DO NOTHING",
        params![item.bank_id, item.created_at],
    )
    .am("bank register")?;
    let sk_rows = tx
        .execute(
            "INSERT INTO subject_keys (subject_id, dek_ref, status, created_at, updated_at)
         VALUES (?1, ?2, 'active', ?3, ?3)
         ON CONFLICT(subject_id) DO UPDATE SET
           dek_ref = excluded.dek_ref,
           status = 'active',
           shredded_at = NULL,
           updated_at = excluded.updated_at
           WHERE subject_keys.shredded_at IS NULL",
            params![item.subject_id, dek_ref, ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows as u64, &item.subject_id)?;
    map_item_insert(
        tx.execute(
            "INSERT INTO items (
                id, bank_id, kind, category, episodic_type, epistemic_kind, source_type,
                confidence, admission_score, source_ref, content_ciphertext, subject_id,
                valid_from, valid_until, tx_from, tx_until, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18
             )",
            params![
                item.id,
                item.bank_id,
                item.kind.as_str(),
                category,
                episodic_type,
                item.epistemic_kind.as_str(),
                source_type,
                item.confidence,
                item.admission_score,
                item.source_ref,
                cipher,
                item.subject_id,
                item.valid_from,
                item.valid_until,
                item.tx_from,
                item.tx_until,
                item.created_at,
                item.updated_at,
            ],
        )
        .map(|n| n as u64)
        .map_err(|e| e.to_string()),
    )?;
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES (?1, ?2, 'create_memory_item', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            format!("audit-{}-{ts}", item.id),
            item.bank_id,
            item.id,
            category,
            item.epistemic_kind.as_str(),
            item.confidence,
            item.admission_score,
            actor,
            crate::item_persist::context_audit_detail(item),
            ts,
        ],
    )
    .am("audit insert")?;
    crate::sqlite_index::enqueue_index_pending_tx(tx, &item.bank_id, &item.id)?;
    Ok(())
}

/// Transaction-scoped active item id for a subject: the single-slot prior a
/// `canonical_put` must close. Resolved inside the batch transaction so two
/// puts on one key in one batch still leave exactly one active revision.
pub(crate) fn active_item_id_by_subject_tx(
    tx: &Transaction<'_>,
    bank_id: &str,
    subject_id: &str,
) -> Result<Option<String>, AmError> {
    tx.query_row(
        "SELECT id FROM items
          WHERE bank_id = ?1 AND subject_id = ?2 AND tx_until IS NULL
          ORDER BY tx_from DESC LIMIT 1",
        params![bank_id, subject_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .am("active item by subject")
}

impl SqliteStore {
    /// Ungated create. Public long-term tools must wrap with admission later.
    pub(crate) fn create_memory_item_impl(
        &self,
        item: &MemoryItem,
        actor: &str,
    ) -> Result<(), AmError> {
        item.validate()?;
        require_actor(actor)?;
        let dek_ref = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("item create begin")?;
            create_memory_item_tx(&tx, item, &cipher, &dek_ref, actor)?;
            tx.commit().am("item create commit")?;
            Ok(())
        })
    }

    /// Ungated update of mutable fields (content halves, scores, provenance).
    pub(crate) fn update_memory_item_impl(
        &self,
        item: &MemoryItem,
        actor: &str,
    ) -> Result<(), AmError> {
        item.validate()?;
        require_actor(actor)?;
        let _ = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("item update begin")?;
            crate::item_update::update_memory_item_sqlite_tx(&tx, item, &cipher, actor)?;
            tx.commit().am("item update commit")?;
            Ok(())
        })
    }

    pub(crate) fn get_memory_item_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<MemoryItem>, AmError> {
        let row = self.with_conn(|conn| {
            conn.query_row(
                ITEM_COLUMNS_QUERY,
                params![item_id, bank_id],
                map_item_columns,
            )
            .optional()
            .am("memory item get")
        })?;
        map_item_row(self.kms().as_ref(), row)
    }

    /// Trait body: latest active (or `as_of`-point-in-time) item row by
    /// subject id — the single-slot canonical read primitive.
    pub(crate) fn get_item_by_subject_impl(
        &self,
        bank_id: &str,
        subject_id: &str,
        as_of: Option<&str>,
    ) -> Result<Option<MemoryItem>, AmError> {
        let (filter, arg): (&str, Option<&str>) = match as_of {
            Some(at) => (
                "tx_from <= ?3 AND (tx_until IS NULL OR tx_until > ?3)",
                Some(at),
            ),
            None => ("tx_until IS NULL", None),
        };
        let sql = format!(
            "{ITEM_COLUMNS_SELECT} WHERE bank_id = ?1 AND subject_id = ?2 AND {filter}
             ORDER BY tx_from DESC LIMIT 1"
        );
        let row = self.with_conn(|conn| match arg {
            Some(at) => conn
                .query_row(&sql, params![bank_id, subject_id, at], map_item_columns)
                .optional()
                .am("item by subject get"),
            None => conn
                .query_row(&sql, params![bank_id, subject_id], map_item_columns)
                .optional()
                .am("item by subject get"),
        })?;
        map_item_row(self.kms().as_ref(), row)
    }

    pub(crate) fn get_snapshot_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<Value>, AmError> {
        match self.get_memory_item_impl(bank_id, item_id)? {
            None => Ok(None),
            Some(item) => match item.snapshot {
                Some(v) => Ok(Some(v)),
                None => Err(snapshot_missing()),
            },
        }
    }

    pub(crate) fn get_gist_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<String>, AmError> {
        match self.get_memory_item_impl(bank_id, item_id)? {
            None => Ok(None),
            Some(item) => match item.gist {
                Some(v) => Ok(Some(v)),
                None => Err(gist_missing()),
            },
        }
    }
}
