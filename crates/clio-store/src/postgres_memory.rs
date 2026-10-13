// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres ungated memory-item repository primitives.
//!
//! ## Owns
//! - Postgres create/update/get for `MemoryItem` and snapshot/gist getters
//! - The subject-keyed bi-temporal read primitive (single-slot canonical reads)
//!
//! ## Does not own
//! - Domain validation (`clio-types`)
//! - DualContent seal helpers (`item_persist`)
//!
//! ## Boundary
//!
//! Changes to Postgres item SQL should normally require changes here.
//! Changes to SQLite item SQL should not require changes here.

use crate::item_persist::{
    gist_missing, kind_columns, missing_ciphertext, open_dual, require_actor, seal_dual,
    snapshot_missing, source_type_sql,
};
use crate::migrate::{IntoAmError, now_iso};
use crate::postgres::PostgresStore;
use clio_types::{
    AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory,
    SourceType,
};
use postgres::error::SqlState;
use serde_json::Value;

/// Map a Postgres item-insert failure: unique violations mean a duplicate id.
///
/// Every other refusal is routed through [`crate::migrate::map_db_err`] rather
/// than formatted here. This function used to interpolate the driver error
/// itself, which for a server-side refusal is the bare string `db error` — so a
/// check-constraint violation, a missing column, and a failed load all reported
/// the same `items insert: db error` with nothing to act on. `map_db_err` walks
/// the driver's source chain, so the constraint name, `DETAIL` and `HINT`
/// survive, and the duplicate case keeps its own deliberate wording above.
pub(crate) fn map_pg_item_insert(result: Result<u64, postgres::Error>) -> Result<(), AmError> {
    match result {
        Ok(_) => Ok(()),
        Err(e) if e.code() == Some(&SqlState::UNIQUE_VIOLATION) => Err(AmError::new(
            ErrorCode::InvalidArgument,
            "duplicate item id",
        )),
        Err(e) => Err(crate::migrate::map_db_err("items insert", e)),
    }
}

/// Shared items projection (Postgres casts for the float columns).
const ITEM_COLUMNS_SELECT: &str = "SELECT id, bank_id, subject_id, kind, category, episodic_type,
        epistemic_kind, source_type, confidence::float8, admission_score::float8, source_ref,
        content_ciphertext, valid_from, valid_until, tx_from, tx_until, created_at, updated_at
 FROM items";

/// Type-map one items row into the decrypted [`MemoryItem`] (or `None`).
fn map_item_row(
    kms: &dyn crate::dek::DekProvider,
    row: Option<postgres::Row>,
) -> Result<Option<MemoryItem>, AmError> {
    let Some(row) = row else {
        return Ok(None);
    };
    let id: String = row.get(0);
    let bank_id: String = row.get(1);
    let subject_id: String = row.get(2);
    let kind: String = row.get(3);
    let category: Option<String> = row.get(4);
    let episodic_type: Option<String> = row.get(5);
    let epistemic_kind: String = row.get(6);
    let source_type: Option<String> = row.get(7);
    let confidence: Option<f64> = row.get(8);
    let admission_score: Option<f64> = row.get(9);
    let source_ref: Option<String> = row.get(10);
    let cipher: Option<String> = row.get(11);
    let valid_from: String = row.get(12);
    let valid_until: Option<String> = row.get(13);
    let tx_from: String = row.get(14);
    let tx_until: Option<String> = row.get(15);
    let created_at: String = row.get(16);
    let updated_at: String = row.get(17);
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
/// atomic batch executor (`batch_postgres`).
pub(crate) fn create_memory_item_tx(
    tx: &mut postgres::Transaction<'_>,
    item: &MemoryItem,
    cipher: &str,
    dek_ref: &str,
    actor: &str,
) -> Result<(), AmError> {
    let (category, episodic_type) = kind_columns(item);
    let cat = category.map(str::to_owned);
    let epi = episodic_type.map(str::to_owned);
    let kind = item.kind.as_str().to_owned();
    let epistemic = item.epistemic_kind.as_str().to_owned();
    let src = source_type_sql(item).map(str::to_owned);
    let ts = now_iso();
    let audit_id = format!("audit-{}-{}", item.id, ts);
    tx.execute(
        "INSERT INTO banks (id, created_at, updated_at) VALUES ($1, $2, $2)
         ON CONFLICT (id) DO NOTHING",
        &[&item.bank_id, &item.created_at],
    )
    .am("bank register")?;
    let sk_rows = tx
        .execute(
            "INSERT INTO subject_keys (subject_id, dek_ref, status, created_at, updated_at)
         VALUES ($1, $2, 'active', $3, $3)
         ON CONFLICT (subject_id) DO UPDATE SET
           dek_ref = EXCLUDED.dek_ref,
           status = 'active',
           shredded_at = NULL,
           updated_at = EXCLUDED.updated_at
           WHERE subject_keys.shredded_at IS NULL",
            &[&item.subject_id, &dek_ref, &ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows, &item.subject_id)?;
    map_pg_item_insert(tx.execute(
        "INSERT INTO items (
            id, bank_id, kind, category, episodic_type, epistemic_kind, source_type,
            confidence, admission_score, source_ref, content_ciphertext, subject_id,
            valid_from, valid_until, tx_from, tx_until, created_at, updated_at
         ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8::float8, $9::float8, $10, $11, $12,
            $13, $14, $15, $16, $17, $18
         )",
        &[
            &item.id,
            &item.bank_id,
            &kind,
            &cat,
            &epi,
            &epistemic,
            &src,
            &item.confidence,
            &item.admission_score,
            &item.source_ref,
            &cipher,
            &item.subject_id,
            &item.valid_from,
            &item.valid_until,
            &item.tx_from,
            &item.tx_until,
            &item.created_at,
            &item.updated_at,
        ],
    ))?;
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES ($1, $2, 'create_memory_item', $3, $4, $5, $6::float8, $7::float8, $8, $9, $10)",
        &[
            &audit_id,
            &item.bank_id,
            &item.id,
            &cat,
            &epistemic,
            &item.confidence,
            &item.admission_score,
            &actor,
            &crate::item_persist::context_audit_detail(item),
            &ts,
        ],
    )
    .am("audit insert")?;
    crate::postgres_index::enqueue_index_pending_tx(tx, &item.bank_id, &item.id)?;
    Ok(())
}

/// Transaction-scoped active item id for a subject: the single-slot prior a
/// `canonical_put` must close. Resolved inside the batch transaction so two
/// puts on one key in one batch still leave exactly one active revision.
pub(crate) fn active_item_id_by_subject_tx(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    subject_id: &str,
) -> Result<Option<String>, AmError> {
    let row = tx
        .query_opt(
            "SELECT id FROM items
              WHERE bank_id = $1 AND subject_id = $2 AND tx_until IS NULL
              ORDER BY tx_from DESC LIMIT 1",
            &[&bank_id, &subject_id],
        )
        .am("active item by subject")?;
    Ok(row.map(|r| r.get::<_, String>(0)))
}

impl PostgresStore {
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("item create begin")?;
            create_memory_item_tx(&mut tx, item, &cipher, &dek_ref, actor)?;
            tx.commit().am("item create commit")?;
            Ok(())
        })
    }

    pub(crate) fn update_memory_item_impl(
        &self,
        item: &MemoryItem,
        actor: &str,
    ) -> Result<(), AmError> {
        item.validate()?;
        require_actor(actor)?;
        let _ = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("item update begin")?;
            crate::item_update::update_memory_item_postgres_tx(&mut tx, item, &cipher, actor)?;
            tx.commit().am("item update commit")?;
            Ok(())
        })
    }

    pub(crate) fn get_memory_item_impl(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<Option<MemoryItem>, AmError> {
        let row = self.with_client(|client| {
            client
                .query_opt(
                    &format!("{ITEM_COLUMNS_SELECT} WHERE id = $1 AND bank_id = $2"),
                    &[&item_id, &bank_id],
                )
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
                "tx_from <= $3 AND (tx_until IS NULL OR tx_until > $3)",
                Some(at),
            ),
            None => ("tx_until IS NULL", None),
        };
        let sql = format!(
            "{ITEM_COLUMNS_SELECT} WHERE bank_id = $1 AND subject_id = $2 AND {filter}
             ORDER BY tx_from DESC LIMIT 1"
        );
        let row = self.with_client(|client| match arg {
            Some(at) => client
                .query_opt(&sql, &[&bank_id, &subject_id, &at])
                .am("item by subject get"),
            None => client
                .query_opt(&sql, &[&bank_id, &subject_id])
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
