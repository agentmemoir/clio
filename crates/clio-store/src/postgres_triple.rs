// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres atomic triple assert/supersede writes.
//!
//! ## Owns
//! - `commit_triple_add` transaction on Postgres (single-op and batch-shared)
//! - Continuous attribute-schema rejection on the discrete path
//!
//! ## Does not own
//! - Triple query / end / invalidate (see `postgres_triple_read`)
//!
//! ## Boundary
//!
//! Changes to Postgres supersession write SQL should normally require changes here.

use crate::item_persist::{
    kind_columns, map_item_insert, require_actor, seal_dual, source_type_sql,
};
use crate::migrate::{IntoAmError, now_iso};
use crate::postgres::PostgresStore;
use crate::triple_cipher::TripleSeal;
use crate::triple_map::wrong_update_rule_attr;
use clio_types::{AmError, ErrorCode, MemoryItem, TripleAddOutcome, TripleRecord};

/// Transaction-scoped triple commit: continuous-attr guard, prior close,
/// carrier item insert, SPO insert, audit row, and index enqueue. Shared by
/// the single-op path and the atomic batch executor (`batch_postgres`).
///
/// `seal.object` is the asserted VALUE already sealed under
/// `item.subject_id`'s DEK by the caller (see `triple_cipher`); `subject` and
/// `predicate` bind in plaintext because makes them the identity key.
#[allow(clippy::too_many_lines)]
pub(crate) fn commit_triple_add_tx(
    tx: &mut postgres::Transaction<'_>,
    item: &MemoryItem,
    edge: &TripleRecord,
    supersede: bool,
    actor: &str,
    seal: &TripleSeal<'_>,
) -> Result<TripleAddOutcome, AmError> {
    let (category, episodic_type) = kind_columns(item);
    let cat = category.map(str::to_owned);
    let epi = episodic_type.map(str::to_owned);
    let kind = item.kind.as_str();
    let epistemic = item.epistemic_kind.as_str();
    let src = source_type_sql(item).map(str::to_owned);
    let edge_source = edge.source_type.map(|s| s.as_str().to_owned());
    let ts = now_iso();
    let class_row = tx
        .query_opt(
            "SELECT attr_class FROM attribute_schemas
             WHERE bank_id = $1 AND attr_key = $2",
            &[&edge.bank_id, &edge.predicate],
        )
        .am("attr_class lookup")?;
    if class_row.is_some_and(|r| r.get::<_, String>(0) == "continuous") {
        return Err(wrong_update_rule_attr(&edge.predicate));
    }
    if let Some(ref_id) = &edge.snapshot_ref {
        let ok = tx
            .query_opt(
                "SELECT id FROM items WHERE id = $1 AND bank_id = $2",
                &[&ref_id, &edge.bank_id],
            )
            .am("snapshot_ref lookup")?;
        if ok.is_none() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("snapshot_ref `{ref_id}` not found in bank"),
            ));
        }
    }
    // tx-open, not valid-open: an edge with a known valid_until but no
    // tx_until is still the system's current belief and must supersede.
    let open_rows = tx
        .query(
            "SELECT id FROM triples
             WHERE bank_id = $1 AND subject = $2 AND predicate = $3
               AND tx_until IS NULL
             FOR UPDATE",
            &[&edge.bank_id, &edge.subject, &edge.predicate],
        )
        .am("open triples query")?;
    let prior_edge_ids: Vec<String> = open_rows.iter().map(|r| r.get(0)).collect();
    if supersede {
        for pid in &prior_edge_ids {
            tx.execute(
                "UPDATE triples
                 SET valid_until = COALESCE(valid_until, $1), tx_until = $2, updated_at = $3
                 WHERE id = $4 AND bank_id = $5",
                &[&edge.valid_from, &edge.tx_from, &ts, pid, &edge.bank_id],
            )
            .am("close prior triple")?;
        }
    } else if !prior_edge_ids.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "open subject+predicate edge exists; set supersede=true or triple_end first",
        ));
    }
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
            &[&item.subject_id, &seal.dek_ref, &ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows, &item.subject_id)?;
    map_item_insert(
        tx.execute(
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
                &seal.carrier,
                &item.subject_id,
                &item.valid_from,
                &item.valid_until,
                &item.tx_from,
                &item.tx_until,
                &item.created_at,
                &item.updated_at,
            ],
        )
        .map_err(|e| e.to_string()),
    )?;
    tx.execute(
        "INSERT INTO triples (
            id, bank_id, item_id, subject, predicate, object_ciphertext, epistemic_kind,
            source_type, confidence, valid_from, valid_until, tx_from, tx_until, snapshot_ref,
            gist_ref, created_at, updated_at
         ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8,
            $9::float8, $10, $11, $12, $13, $14, $15,
            $16, $17
         )",
        &[
            &edge.id,
            &edge.bank_id,
            &edge.item_id,
            &edge.subject,
            &edge.predicate,
            &seal.object,
            &edge.epistemic_kind.as_str(),
            &edge_source,
            &edge.confidence,
            &edge.valid_from,
            &edge.valid_until,
            &edge.tx_from,
            &edge.tx_until,
            &edge.snapshot_ref,
            &edge.gist_ref,
            &edge.created_at,
            &edge.updated_at,
        ],
    )
    .am("triples insert")?;
    let detail = crate::history_store::triple_audit_detail(item, &edge.id, &prior_edge_ids);
    let audit_id = format!("audit-{}-{}", item.id, ts);
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, category, epistemic_kind,
            confidence, admission_score, actor, detail_json, created_at
         ) VALUES ($1, $2, 'triple_add', $3, $4, $5, $6::float8, $7::float8, $8, $9, $10)",
        &[
            &audit_id,
            &item.bank_id,
            &item.id,
            &cat,
            &epistemic,
            &item.confidence,
            &item.admission_score,
            &actor,
            &detail,
            &ts,
        ],
    )
    .am("audit insert")?;
    crate::postgres_index::enqueue_index_pending_tx(tx, &item.bank_id, &item.id)?;
    Ok(TripleAddOutcome {
        new_edge_id: edge.id.clone(),
        prior_edge_ids,
    })
}

impl PostgresStore {
    pub(crate) fn commit_triple_add_impl(
        &self,
        item: &MemoryItem,
        edge: &TripleRecord,
        supersede: bool,
        actor: &str,
    ) -> Result<TripleAddOutcome, AmError> {
        item.validate()?;
        edge.validate()?;
        require_actor(actor)?;
        if item.id != edge.item_id || item.bank_id != edge.bank_id {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "triple item_id/bank_id must match carrier MemoryItem",
            ));
        }
        if item.episodic_type != Some(clio_types::EpisodicType::Triple) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "triple carrier item must have episodic_type=triple",
            ));
        }
        let dek_ref = self.kms().ensure_dek(&item.subject_id)?;
        let cipher = seal_dual(self.kms().as_ref(), &item.subject_id, item)?;
        let object_cipher =
            crate::triple_cipher::seal_object(self.kms().as_ref(), &item.subject_id, &edge.object)?;
        let seal = TripleSeal {
            carrier: &cipher,
            object: &object_cipher,
            dek_ref: &dek_ref,
        };
        self.with_client(|client| {
            let mut tx = client.transaction().am("triple add begin")?;
            let outcome = commit_triple_add_tx(&mut tx, item, edge, supersede, actor, &seal)?;
            tx.commit().am("triple add commit")?;
            Ok(outcome)
        })
    }
}

#[cfg(test)]
#[path = "postgres_triple_open_uniq_tests.rs"]
mod postgres_triple_open_uniq_tests;

#[cfg(test)]
#[path = "postgres_triple_reshape_tests.rs"]
mod postgres_triple_reshape_tests;

#[cfg(test)]
#[path = "postgres_triple_seal_erase_tests.rs"]
pub(crate) mod postgres_triple_seal_erase_tests;

#[cfg(test)]
#[path = "postgres_triple_tests.rs"]
mod postgres_triple_tests;
