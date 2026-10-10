// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for extracted Postgres sync SQL bodies behind
//! the `SyncStore` impl: the per-bank journal append
//! transaction, the watermark change feeds for items, association edges, and
//! triples, and the versioned change feeds for the context-bearing domain
//! records (persona stable entries, persona preferences, beliefs) with their
//! opened-plaintext row mapping.
//!
//! ## Owns
//! - Postgres SQL bodies for `SyncStore` change feeds and journal append
//!
//! ## Does not own
//! - The `SyncStore` impl block (see `postgres_sync`)
//! - Persona/belief repository SQL (`postgres_persona`, `postgres_belief`)
//! - Apply semantics (see `clio-sync`)
//!
//! ## Boundary
//!
//! Changes to Postgres sync feed SQL should normally require changes here.
//! Changes to apply or conflict semantics should not require changes here.
//!
//! Keep this module focused on the Postgres sync feed surface.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::postgres_sync_rows::limit_i64;
use crate::sync_store_rows::split_push_watermark;
use clio_types::{AmError, BeliefObject, ConfidenceEntry, PersonaPreference, PersonaStableEntry};

/// Per-belief cap on the confidence entries a single feed page may load (see
/// the SQLite mirror): oldest-first so a capped read stays a stored prefix.
const MAX_BELIEF_TRAJECTORY: i64 = 1000;

/// Postgres body of `SyncStore::journal_append` (idempotent per-bank append).
pub(crate) fn journal_append_pg(
    store: &PostgresStore,
    row: &crate::sync_store::SyncJournalRow,
) -> Result<crate::sync_store::JournalAppendOutcome, AmError> {
    use crate::sync_store::JournalAppendOutcome;
    store.with_client(|client| {
        let mut tx = client.transaction().am("sync journal begin")?;
        if let Some(existing) = tx
            .query_opt(
                "SELECT seq FROM sync_mutations WHERE bank_id = $1 AND sync_mutation_id = $2",
                &[&row.bank_id, &row.sync_mutation_id],
            )
            .am("sync journal dedupe probe")?
        {
            let seq: i32 = existing.get(0);
            tx.commit().am("sync journal commit (no-op)")?;
            return Ok(JournalAppendOutcome {
                seq: i64::from(seq),
                inserted: false,
            });
        }
        let seq: i32 = tx
            .query_one(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM sync_mutations WHERE bank_id = $1",
                &[&row.bank_id],
            )
            .am("sync journal next seq")?
            .get(0);
        let encrypted = i32::from(row.client_encrypted);
        tx.execute(
            "INSERT INTO sync_mutations (bank_id, seq, sync_mutation_id, entity_kind, \
             entity_id, op, transaction_time, device_id, content_hash, content_sealed, \
             encrypted, admission_receipt, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            &[
                &row.bank_id,
                &seq,
                &row.sync_mutation_id,
                &row.entity_kind,
                &row.entity_id,
                &row.op,
                &row.transaction_time,
                &row.device_id,
                &row.content_hash,
                &row.content_sealed,
                &encrypted,
                &row.admission_receipt,
                &row.created_at,
            ],
        )
        .am("sync journal insert")?;
        tx.commit().am("sync journal commit")?;
        Ok(JournalAppendOutcome {
            seq: i64::from(seq),
            inserted: true,
        })
    })
}

/// Postgres body of `SyncStore::feed_items`.
pub(crate) fn feed_items_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<crate::sync_store::SyncFeedRow>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT id, tx_from, tx_until, discarded_at, \
                 COALESCE(updated_at, created_at) AS stamp FROM items WHERE bank_id = $1 \
                 AND COALESCE(episodic_type, '') NOT IN ('task', 'failure') \
                 AND ($2 = '1970-01-01T00:00:00Z' OR tx_from > $2 \
                      OR COALESCE(tx_until, '') > $2 \
                      OR COALESCE(discarded_at, '') > $2 \
                      OR COALESCE(updated_at, created_at) > $2 \
                      OR (COALESCE(updated_at, created_at) = $2 AND id > $3)) \
                 ORDER BY stamp, id LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed items")?;
        Ok(rows
            .iter()
            .map(|r| {
                crate::postgres_sync_rows::feed_item_row(
                    r.get::<_, String>(0).as_str(),
                    r.get::<_, String>(1).as_str(),
                    r.get::<_, Option<String>>(2).as_deref(),
                    r.get::<_, Option<String>>(3).as_deref(),
                    r.get::<_, String>(4).as_str(),
                )
            })
            .collect())
    })
}

/// Postgres body of `SyncStore::feed_assoc`.
pub(crate) fn feed_assoc_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<crate::sync_store::SyncFeedRow>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT id, last_updated_at FROM assoc_edges \
                 WHERE bank_id = $1 AND ($2 = '1970-01-01T00:00:00Z' OR last_updated_at > $2 \
                      OR (last_updated_at = $2 AND id > $3)) \
                 ORDER BY 2, id LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed assoc")?;
        Ok(rows
            .iter()
            .map(|r| crate::sync_store::SyncFeedRow {
                entity_kind: "assoc_edge".into(),
                entity_id: r.get(0),
                op: "upsert".into(),
                transaction_time: r.get(1),
            })
            .collect())
    })
}

/// Postgres body of `SyncStore::feed_triples`.
pub(crate) fn feed_triples_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<crate::sync_store::SyncFeedRow>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT id, COALESCE(tx_until, valid_until, '') FROM triples \
                 WHERE bank_id = $1 \
                 AND ($2 = '1970-01-01T00:00:00Z' OR COALESCE(tx_until, '') > $2 \
                      OR COALESCE(valid_until, '') > $2 \
                      OR COALESCE(tx_until, valid_until, '') > $2 \
                      OR (COALESCE(tx_until, valid_until, '') = $2 AND id > $3)) \
                 ORDER BY COALESCE(tx_until, valid_until, ''), id LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed triples")?;
        Ok(rows
            .iter()
            .map(|r| crate::sync_store::SyncFeedRow {
                entity_kind: "triple".into(),
                entity_id: r.get(0),
                op: "invalidate".into(),
                transaction_time: r.get(1),
            })
            .collect())
    })
}

/// Postgres body of `SyncStore::feed_persona_stable`: the latest open version
/// per `(bank, attr_key)`, opened plaintext, with its change stamp.
pub(crate) fn feed_persona_stable_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, PersonaStableEntry)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT p.attr_key, p.value_cipher, p.context_cipher, p.confidence::float8, \
                 p.source, p.source_ref, p.admission_score::float8, p.valid_from, \
                 COALESCE(p.updated_at, p.created_at) FROM persona_stable_entries p \
                 WHERE p.bank_id = $1 AND p.valid_until IS NULL \
                 AND ($2 = '1970-01-01T00:00:00Z' OR COALESCE(p.updated_at, p.created_at) > $2 \
                      OR (COALESCE(p.updated_at, p.created_at) = $2 AND p.attr_key > $3)) \
                 AND NOT EXISTS ( \
                 SELECT 1 FROM persona_stable_entries q WHERE q.bank_id = p.bank_id \
                 AND q.attr_key = p.attr_key AND (COALESCE(q.updated_at, q.created_at) > \
                 COALESCE(p.updated_at, p.created_at) OR (COALESCE(q.updated_at, q.created_at) = \
                 COALESCE(p.updated_at, p.created_at) AND q.id > p.id))) \
                 ORDER BY COALESCE(p.updated_at, p.created_at), p.attr_key LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed persona stable")?;
        let mut out = Vec::new();
        for row in &rows {
            let key: String = row.get(0);
            let value_cipher: String = row.get(1);
            let context_cipher: Option<String> = row.get(2);
            out.push((
                row.get(8),
                PersonaStableEntry {
                    bank_id: bank_id.to_owned(),
                    key,
                    value: crate::persona_store::open_persona_value(
                        store.kms().as_ref(),
                        &value_cipher,
                    )?,
                    context: crate::persona_store::open_persona_context(
                        store.kms().as_ref(),
                        context_cipher.as_deref(),
                    ),
                    confidence: row.get(3),
                    source: row.get(4),
                    source_ref: row.get(5),
                    admission_score: row.get(6),
                    as_of: row.get(7),
                    valid_until: None,
                },
            ));
        }
        Ok(out)
    })
}

/// Postgres body of `SyncStore::feed_persona_preferences`.
pub(crate) fn feed_persona_prefs_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, PersonaPreference)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT attr_key, value::float8, trend::float8, context_cipher, \
                 confidence::float8, source, source_ref, admission_score::float8, as_of, \
                 COALESCE(updated_at, created_at) FROM persona_preferences \
                 WHERE bank_id = $1 \
                 AND ($2 = '1970-01-01T00:00:00Z' OR COALESCE(updated_at, created_at) > $2 \
                      OR (COALESCE(updated_at, created_at) = $2 AND attr_key > $3)) \
                 ORDER BY 10, attr_key LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed persona prefs")?;
        let mut out = Vec::new();
        for row in &rows {
            let key: String = row.get(0);
            let context_cipher: Option<String> = row.get(3);
            out.push((
                row.get(9),
                PersonaPreference {
                    bank_id: bank_id.to_owned(),
                    key,
                    value: row.get(1),
                    trend: row.get(2),
                    confidence: row.get(4),
                    source: row.get(5),
                    source_ref: row.get(6),
                    context: crate::persona_store::open_persona_context(
                        store.kms().as_ref(),
                        context_cipher.as_deref(),
                    ),
                    admission_score: row.get(7),
                    as_of: row.get(8),
                },
            ));
        }
        Ok(out)
    })
}

/// Postgres body of `SyncStore::feed_beliefs`: opened propositions, contexts,
/// and full trajectories with each row's change stamp.
pub(crate) fn feed_beliefs_pg(
    store: &PostgresStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, BeliefObject)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_client(|client| {
        let rows = client
            .query(
                "SELECT id, bank_id, subject_id, proposition_ciphertext, context_cipher, \
                 context_as_of, created_at, COALESCE(updated_at, created_at) FROM beliefs \
                 WHERE bank_id = $1 \
                 AND ($2 = '1970-01-01T00:00:00Z' OR COALESCE(updated_at, created_at) > $2 \
                      OR (COALESCE(updated_at, created_at) = $2 AND id > $3)) \
                 ORDER BY 8, id LIMIT $4",
                &[&bank_id, &since_stamp, &since_entity, &limit_i64(limit)],
            )
            .am("sync feed beliefs")?;
        // Trajectories read on the same client: `with_client` is not
        // re-entrant, so nested store calls would deadlock here. The load is
        // bounded to this page's belief ids AND a per-belief cap, so one feed
        // page can never pull an unbounded number of confidence entries.
        let ids: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
        let mapped = if ids.is_empty() {
            Vec::new()
        } else {
            client
                .query(
                    "SELECT belief_id, as_of, confidence, evidence_ref, source_type FROM ( \
                     SELECT e.belief_id AS belief_id, e.as_of AS as_of, \
                     e.confidence::float8 AS confidence, e.evidence_ref AS evidence_ref, \
                     e.source_type AS source_type, \
                     ROW_NUMBER() OVER (PARTITION BY e.belief_id ORDER BY e.as_of, e.id) AS rn \
                     FROM belief_confidence_entries e WHERE e.belief_id = ANY($1)) t \
                     WHERE rn <= $2 ORDER BY belief_id, as_of",
                    &[&ids, &MAX_BELIEF_TRAJECTORY],
                )
                .am("sync feed belief entries")?
        };
        let entries: Vec<(String, ConfidenceEntry)> = mapped
            .iter()
            .map(|r| {
                Ok((
                    r.get::<_, String>(0),
                    ConfidenceEntry {
                        as_of: r.get(1),
                        confidence: r.get(2),
                        evidence_ref: r.get(3),
                        source_type: clio_types::SourceType::parse(&r.get::<_, String>(4))?,
                    },
                ))
            })
            .collect::<Result<Vec<_>, AmError>>()
            .am("sync feed belief entries parse")?;
        let mut out = Vec::new();
        for row in &rows {
            let id: String = row.get(0);
            let context_cipher: Option<String> = row.get(4);
            let context_as_of: Option<String> = row.get(5);
            let created_at: String = row.get(6);
            let proposition = crate::belief_store::open_proposition(
                store.kms().as_ref(),
                &row.get::<_, String>(3),
            )?;
            let history: Vec<ConfidenceEntry> = entries
                .iter()
                .filter(|(belief_id, _)| belief_id == &id)
                .map(|(_, entry)| entry.clone())
                .collect();
            let updated_at = history
                .last()
                .map(|entry| entry.as_of.clone())
                .unwrap_or_else(|| created_at.clone());
            out.push((
                row.get::<_, String>(7),
                BeliefObject {
                    id,
                    bank_id: bank_id.to_owned(),
                    subject_id: row.get(2),
                    proposition,
                    context: crate::belief_store::open_belief_context(
                        store.kms().as_ref(),
                        context_cipher.as_deref(),
                    ),
                    context_as_of,
                    confidence_history: history,
                    created_at,
                    updated_at,
                },
            ));
        }
        Ok(out)
    })
}

#[cfg(test)]
#[path = "postgres_sync_records_tests.rs"]
mod postgres_sync_records_tests;
