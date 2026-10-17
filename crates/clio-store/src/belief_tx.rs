// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the transaction-scoped belief create/append
//! bodies shared by both backends: belief identity insert (with sealed optional
//! context), confidence-trajectory inserts, `updated_at` sync, and the belief
//! audit row with hash-only context telemetry.
//!
//! ## Owns
//! - `create_belief_*_tx` / `append_confidence_*_tx` SQL for SQLite and Postgres
//! - Sealed-context persistence, the store-derived `context_as_of` stamp, the
//!   monotonic `updated_at` sync, and context-telemetry merge inside those txs
//! - The shared `validate_belief_create` / `validate_append` validators the
//!   per-backend impls and batch executors call before their transactions
//!
//! ## Does not own
//! - The `BeliefStore` contract or validation (`belief_store`)
//! - The trajectory-row INSERT bodies (`belief_entry_insert`)
//! - Sealing/opening ciphertext (callers seal before calling)
//!
//! ## Boundary
//!
//! Changes to belief-row SQL should normally require changes here. Changes to
//! the belief repository surface should require `belief_store`.
//!
//! Keep this module focused on belief write transaction bodies.

use crate::belief_store::{
    belief_audit_detail, belief_context_stats, map_belief_insert, require_nonempty, unique_entry_id,
};
use crate::migrate::{IntoAmError, now_iso};
use clio_types::{AmError, BeliefObject, ConfidenceEntry, ErrorCode};
use postgres::Transaction as PgTransaction;

/// Validate the belief identity, trajectory, and optional context shared by
/// both backends.
///
/// Callers use this before opening their transaction so both backends reject
/// the same malformed input with the same error.
pub(crate) fn validate_belief_create(belief: &BeliefObject, actor: &str) -> Result<(), AmError> {
    belief.validate()?;
    crate::item_persist::require_actor(actor)?;
    if belief.confidence_history.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "belief create requires at least one confidence entry",
        ));
    }
    Ok(())
}

/// Validate an append request shared by both backends.
pub(crate) fn validate_append(
    bank_id: &str,
    entry: &ConfidenceEntry,
    actor: &str,
) -> Result<(), AmError> {
    entry.validate()?;
    crate::item_persist::require_actor(actor)?;
    require_nonempty(bank_id, "bank_id")?;
    Ok(())
}

/// Create a belief row + initial trajectory + audit on SQLite.
pub(crate) fn create_belief_sqlite_tx(
    tx: &rusqlite::Transaction<'_>,
    belief: &BeliefObject,
    cipher: &str,
    context_cipher: Option<&str>,
    dek_ref: &str,
    key: &str,
    actor: &str,
) -> Result<(), AmError> {
    use rusqlite::params;
    let ts = now_iso();
    let audit_id = format!("audit-{}-{ts}", belief.id);
    let entries: Vec<(String, ConfidenceEntry)> = belief
        .confidence_history
        .iter()
        .map(|e| (unique_entry_id(&belief.id, &e.as_of), e.clone()))
        .collect();
    let updated_at = newest_entry(belief).as_of.clone();
    let last_entry = newest_entry(belief);
    // The store derives the context effective time: a belief created with a
    // context carries it from `created_at`. A caller-supplied `context_as_of`
    // is ignored — sync apply must not be able to forge the stamp.
    let context_as_of = context_cipher.map(|_| belief.created_at.as_str());
    let context_stats = belief_context_stats(belief.context.as_deref());
    let audit_detail = belief_audit_detail(
        "created",
        last_entry.confidence,
        last_entry.source_type.as_str(),
        belief.confidence_history.len(),
        Some(&context_stats),
    );
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
            params![belief.subject_id, dek_ref, ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows as u64, &belief.subject_id)?;
    map_belief_insert(
        tx.execute(
            "INSERT INTO beliefs (
                id, bank_id, subject_id, proposition_key, proposition_ciphertext,
                context_cipher, context_as_of, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                belief.id,
                belief.bank_id,
                belief.subject_id,
                key,
                cipher,
                context_cipher,
                context_as_of,
                belief.created_at,
                updated_at,
            ],
        )
        .map(|n| n as u64)
        .map_err(|e| e.to_string()),
    )?;
    for (entry_id, entry) in &entries {
        insert_entry_sqlite(tx, entry_id, &belief.id, entry, &ts)?;
    }
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, epistemic_kind, confidence,
            actor, detail_json, created_at
         ) VALUES (?1, ?2, 'belief_observe', ?3, 'belief', ?4, ?5, ?6, ?7)",
        params![
            audit_id,
            belief.bank_id,
            belief.id,
            last_entry.confidence,
            actor,
            audit_detail,
            ts,
        ],
    )
    .am("belief audit insert")?;
    Ok(())
}

/// Append one confidence entry + `updated_at` sync + audit on SQLite.
///
/// `context_cipher` / `context_stats` are `None` when the caller omitted a
/// context: the stored context is preserved and no context telemetry is added.
pub(crate) fn append_confidence_sqlite_tx(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    belief_id: &str,
    entry: &ConfidenceEntry,
    actor: &str,
    context_cipher: Option<&str>,
    context_stats: Option<&serde_json::Value>,
) -> Result<(), AmError> {
    use rusqlite::{OptionalExtension, params};
    let ts = now_iso();
    let entry_id = unique_entry_id(belief_id, &entry.as_of);
    let audit_id = format!("audit-{belief_id}-{entry_id}");
    let exists: Option<String> = tx
        .query_row(
            "SELECT id FROM beliefs WHERE id = ?1 AND bank_id = ?2",
            params![belief_id, bank_id],
            |r| r.get(0),
        )
        .optional()
        .am("belief exists")?;
    if exists.is_none() {
        return Err(belief_not_found(belief_id));
    }
    if let Some(cipher) = context_cipher {
        tx.execute(
            "UPDATE beliefs SET context_cipher = ?1 WHERE id = ?2 AND bank_id = ?3",
            params![cipher, belief_id, bank_id],
        )
        .am("belief context replace")?;
    }
    insert_entry_sqlite(tx, &entry_id, belief_id, entry, &ts)?;
    // MAX keeps the change stamp monotonic: an entry whose `as_of` is older
    // than the current stamp (for example a late-arriving observation after a
    // correction) must never move `updated_at` backwards, or the change feed
    // (`COALESCE(updated_at, created_at) > watermark`) stops selecting the row.
    tx.execute(
        "UPDATE beliefs
         SET updated_at = MAX(COALESCE(updated_at, created_at),
                              (SELECT MAX(as_of) FROM belief_confidence_entries
                               WHERE belief_id = ?1))
         WHERE id = ?1 AND bank_id = ?2",
        params![belief_id, bank_id],
    )
    .am("belief updated_at sync")?;
    let entry_count: usize = tx
        .query_row(
            "SELECT COUNT(*) FROM belief_confidence_entries WHERE belief_id = ?1",
            params![belief_id],
            |r| r.get(0),
        )
        .am("belief entry count")?;
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, epistemic_kind, confidence,
            actor, detail_json, created_at
         ) VALUES (?1, ?2, 'belief_observe', ?3, 'belief', ?4, ?5, ?6, ?7)",
        params![
            audit_id,
            bank_id,
            belief_id,
            entry.confidence,
            actor,
            belief_audit_detail(
                "appended",
                entry.confidence,
                entry.source_type.as_str(),
                entry_count,
                context_stats,
            ),
            ts,
        ],
    )
    .am("belief audit append")?;
    Ok(())
}

/// Create a belief row + initial trajectory + audit on Postgres.
pub(crate) fn create_belief_postgres_tx(
    tx: &mut PgTransaction<'_>,
    belief: &BeliefObject,
    cipher: &str,
    context_cipher: Option<&str>,
    dek_ref: &str,
    key: &str,
    actor: &str,
) -> Result<(), AmError> {
    crate::postgres_memory::lock_subject_write_tx(tx, &belief.subject_id)?;
    let ts = now_iso();
    let audit_id = format!("audit-{}-{ts}", belief.id);
    let entries: Vec<(String, ConfidenceEntry)> = belief
        .confidence_history
        .iter()
        .map(|e| (unique_entry_id(&belief.id, &e.as_of), e.clone()))
        .collect();
    let updated_at = newest_entry(belief).as_of.clone();
    let last_entry = newest_entry(belief);
    // The store derives the context effective time: a belief created with a
    // context carries it from `created_at`. A caller-supplied `context_as_of`
    // is ignored — sync apply must not be able to forge the stamp.
    let context_as_of = context_cipher.map(|_| belief.created_at.as_str());
    let context_stats = belief_context_stats(belief.context.as_deref());
    let audit_detail = belief_audit_detail(
        "created",
        last_entry.confidence,
        last_entry.source_type.as_str(),
        belief.confidence_history.len(),
        Some(&context_stats),
    );
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
            &[&belief.subject_id, &dek_ref, &ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows, &belief.subject_id)?;
    map_belief_insert(
        tx.execute(
            "INSERT INTO beliefs (
                id, bank_id, subject_id, proposition_key, proposition_ciphertext,
                context_cipher, context_as_of, created_at, updated_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                &belief.id,
                &belief.bank_id,
                &belief.subject_id,
                &key,
                &cipher,
                &context_cipher,
                &context_as_of,
                &belief.created_at,
                &updated_at,
            ],
        )
        .map_err(|e| e.to_string()),
    )?;
    for (entry_id, entry) in &entries {
        insert_entry_pg(tx, entry_id, &belief.id, entry, &ts)?;
    }
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, epistemic_kind, confidence,
            actor, detail_json, created_at
         ) VALUES ($1, $2, 'belief_observe', $3, 'belief', $4::float8, $5, $6, $7)",
        &[
            &audit_id,
            &belief.bank_id,
            &belief.id,
            &last_entry.confidence,
            &actor,
            &audit_detail,
            &ts,
        ],
    )
    .am("belief audit insert")?;
    Ok(())
}

/// Append one confidence entry + `updated_at` sync + audit on Postgres.
///
/// `context_cipher` / `context_stats` are `None` when the caller omitted a
/// context: the stored context is preserved and no context telemetry is added.
pub(crate) fn append_confidence_postgres_tx(
    tx: &mut PgTransaction<'_>,
    bank_id: &str,
    belief_id: &str,
    entry: &ConfidenceEntry,
    actor: &str,
    context_cipher: Option<&str>,
    context_stats: Option<&serde_json::Value>,
) -> Result<(), AmError> {
    let ts = now_iso();
    let entry_id = unique_entry_id(belief_id, &entry.as_of);
    let audit_id = format!("audit-{belief_id}-{entry_id}");
    let exists: Option<String> = tx
        .query_opt(
            "SELECT id FROM beliefs WHERE id = $1 AND bank_id = $2",
            &[&belief_id, &bank_id],
        )
        .am("belief exists")?
        .map(|r| r.get(0));
    if exists.is_none() {
        return Err(belief_not_found(belief_id));
    }
    if let Some(cipher) = context_cipher {
        tx.execute(
            "UPDATE beliefs SET context_cipher = $1 WHERE id = $2 AND bank_id = $3",
            &[&cipher, &belief_id, &bank_id],
        )
        .am("belief context replace")?;
    }
    insert_entry_pg(tx, &entry_id, belief_id, entry, &ts)?;
    // GREATEST keeps the change stamp monotonic: an entry whose `as_of` is
    // older than the current stamp (for example a late-arriving observation
    // after a correction) must never move `updated_at` backwards, or the
    // change feed (`COALESCE(updated_at, created_at) > watermark`) stops
    // selecting the row.
    tx.execute(
        "UPDATE beliefs
         SET updated_at = GREATEST(COALESCE(updated_at, created_at),
                                   (SELECT MAX(as_of) FROM belief_confidence_entries
                                    WHERE belief_id = $1))
         WHERE id = $1 AND bank_id = $2",
        &[&belief_id, &bank_id],
    )
    .am("belief updated_at sync")?;
    let entry_count: usize = tx
        .query_one(
            "SELECT COUNT(*)::bigint FROM belief_confidence_entries WHERE belief_id = $1",
            &[&belief_id],
        )
        .am("belief entry count")?
        .get::<_, i64>(0)
        .try_into()
        .expect("trajectory length fits usize");
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, epistemic_kind, confidence,
            actor, detail_json, created_at
         ) VALUES ($1, $2, 'belief_observe', $3, 'belief', $4::float8, $5, $6, $7)",
        &[
            &audit_id,
            &bank_id,
            &belief_id,
            &entry.confidence,
            &actor,
            &belief_audit_detail(
                "appended",
                entry.confidence,
                entry.source_type.as_str(),
                entry_count,
                context_stats,
            ),
            &ts,
        ],
    )
    .am("belief audit append")?;
    Ok(())
}

pub(crate) fn belief_not_found(belief_id: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!("belief `{belief_id}` not found in bank"),
    )
}

/// Newest trajectory entry by `as_of`.
fn newest_entry(belief: &BeliefObject) -> &ConfidenceEntry {
    belief
        .confidence_history
        .iter()
        .max_by(|a, b| a.as_of.cmp(&b.as_of))
        .expect("create validates a non-empty trajectory")
}

#[path = "belief_entry_insert.rs"]
mod belief_entry_insert;

use self::belief_entry_insert::{insert_entry_pg, insert_entry_sqlite};
