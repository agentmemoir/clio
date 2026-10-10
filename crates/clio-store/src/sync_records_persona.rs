// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the extracted SQLite sync SQL bodies for the
//! context-bearing domain-record change feeds: persona
//! stable entries, persona preferences, and beliefs, with their
//! opened-plaintext row mapping and bounded belief-trajectory load.
//!
//! ## Owns
//! - SQLite SQL bodies for the persona/belief sync change feeds
//!
//! ## Does not own
//! - Item/assoc/triple feed and journal-append SQL (`sync_records`)
//! - The `SyncStore` impl block (see `sqlite_sync`)
//! - Persona/belief repository SQL (`sqlite_persona`, `sqlite_belief`)
//! - Apply semantics (see `clio-sync`)
//!
//! ## Boundary
//!
//! Changes to persona/belief sync feed SQL should normally require changes
//! here. Changes to apply or conflict semantics should not require changes here.
//!
//! Keep this module focused on the persona/belief sync feed SQL.

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::sync_store_rows::split_push_watermark;
use clio_types::{AmError, BeliefObject, ConfidenceEntry, PersonaPreference, PersonaStableEntry};
use rusqlite::params;

/// Per-belief cap on the confidence entries a single feed page may load. The
/// trajectory is taken oldest-first so a capped read stays a prefix of the
/// stored trajectory — a later apply then merges idempotently instead of
/// reading the truncated tail as a divergence.
const MAX_BELIEF_TRAJECTORY: i64 = 1000;

/// The empty-watermark sentinel (`1970-01-01T00:00:00Z`) is matched exactly so
/// record stamps in any store-local text format (ISO or epoch millis) never
/// compare below it. All three feeds bind `since` as a compound
/// `(stamp, entity)` pair and resume strictly after `(stamp, tiebreak id)`:
/// attr key for the persona feeds, belief id for beliefs.
/// Change-feed SQL: the latest open persona stable version per
/// `(bank, attr_key)` whose change stamp moved past a watermark. The change
/// stamp is `COALESCE(updated_at, created_at)`.
const FEED_PERSONA_STABLE_SQL: &str = "SELECT p.attr_key, p.value_cipher, p.context_cipher, \
  p.confidence, p.source, p.source_ref, p.admission_score, p.valid_from, \
  COALESCE(p.updated_at, p.created_at) FROM persona_stable_entries p \
  WHERE p.bank_id = ?1 AND p.valid_until IS NULL \
  AND (?2 = '1970-01-01T00:00:00Z' OR COALESCE(p.updated_at, p.created_at) > ?2 \
  OR (COALESCE(p.updated_at, p.created_at) = ?2 AND p.attr_key > ?3)) \
  AND NOT EXISTS ( \
  SELECT 1 FROM persona_stable_entries q WHERE q.bank_id = p.bank_id \
  AND q.attr_key = p.attr_key AND (COALESCE(q.updated_at, q.created_at) > \
  COALESCE(p.updated_at, p.created_at) OR (COALESCE(q.updated_at, q.created_at) = \
  COALESCE(p.updated_at, p.created_at) AND q.id > p.id))) \
  ORDER BY COALESCE(p.updated_at, p.created_at), p.attr_key LIMIT ?4";

/// Change-feed SQL: persona preference rows past a watermark (compound
/// `(stamp, attr_key)` resume).
const FEED_PERSONA_PREFS_SQL: &str = "SELECT attr_key, value, trend, context_cipher, confidence, \
  source, source_ref, admission_score, as_of, COALESCE(updated_at, created_at) \
  FROM persona_preferences WHERE bank_id = ?1 \
  AND (?2 = '1970-01-01T00:00:00Z' OR COALESCE(updated_at, created_at) > ?2 \
  OR (COALESCE(updated_at, created_at) = ?2 AND attr_key > ?3)) \
  ORDER BY 10, attr_key LIMIT ?4";

/// Change-feed SQL: belief rows past a watermark (compound
/// `(stamp, belief id)` resume). `context_as_of` travels with the opened
/// context so peers keep the store-derived effective-from stamp.
const FEED_BELIEFS_SQL: &str = "SELECT id, bank_id, subject_id, proposition_ciphertext, \
  context_cipher, context_as_of, created_at, COALESCE(updated_at, created_at) FROM beliefs \
  WHERE bank_id = ?1 \
  AND (?2 = '1970-01-01T00:00:00Z' OR COALESCE(updated_at, created_at) > ?2 \
  OR (COALESCE(updated_at, created_at) = ?2 AND id > ?3)) \
  ORDER BY 8, id LIMIT ?4";

/// SQLite body of `SyncStore::feed_persona_stable`: the latest open version
/// per `(bank, attr_key)`, opened plaintext, with its change stamp.
pub(crate) fn feed_persona_stable_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, PersonaStableEntry)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(FEED_PERSONA_STABLE_SQL)
            .am("sync feed persona stable prepare")?;
        let rows = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<f64>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, f64>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, String>(8)?,
                    ))
                },
            )
            .am("sync feed persona stable query")?;
        let mut out = Vec::new();
        for row in rows {
            let (
                key,
                value_cipher,
                context_cipher,
                confidence,
                source,
                source_ref,
                admission_score,
                as_of,
                stamp,
            ) = row.am("sync feed persona stable row")?;
            out.push((
                stamp,
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
                    confidence,
                    source,
                    source_ref,
                    admission_score,
                    as_of,
                    valid_until: None,
                },
            ));
        }
        Ok(out)
    })
}

/// SQLite body of `SyncStore::feed_persona_preferences`.
pub(crate) fn feed_persona_prefs_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, PersonaPreference)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(FEED_PERSONA_PREFS_SQL)
            .am("sync feed persona prefs prepare")?;
        let rows = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, f64>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<f64>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, f64>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, String>(9)?,
                    ))
                },
            )
            .am("sync feed persona prefs query")?;
        let mut out = Vec::new();
        for row in rows {
            let (
                key,
                value,
                trend,
                context_cipher,
                confidence,
                source,
                source_ref,
                admission_score,
                as_of,
                stamp,
            ) = row.am("sync feed persona prefs row")?;
            out.push((
                stamp,
                PersonaPreference {
                    bank_id: bank_id.to_owned(),
                    key,
                    value,
                    trend,
                    confidence,
                    source,
                    source_ref,
                    context: crate::persona_store::open_persona_context(
                        store.kms().as_ref(),
                        context_cipher.as_deref(),
                    ),
                    admission_score,
                    as_of,
                },
            ));
        }
        Ok(out)
    })
}

/// Load the per-belief confidence trajectories for a feed page's belief ids.
///
/// Bounded by [`MAX_BELIEF_TRAJECTORY`] per belief and ordered like the
/// stored history, so one feed page can never pull an unbounded number of
/// confidence entries.
fn load_belief_trajectories(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<Vec<(String, String, f64, String, String)>, AmError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT belief_id, as_of, confidence, evidence_ref, source_type FROM ( \
         SELECT e.belief_id AS belief_id, e.as_of AS as_of, e.confidence AS confidence, \
         e.evidence_ref AS evidence_ref, e.source_type AS source_type, \
         ROW_NUMBER() OVER (PARTITION BY e.belief_id ORDER BY e.as_of, e.id) AS rn \
         FROM belief_confidence_entries e WHERE e.belief_id IN ({placeholders})) \
         WHERE rn <= ? ORDER BY belief_id, as_of"
    );
    let mut stmt = conn.prepare(&sql).am("sync feed belief entries prepare")?;
    let mut binds: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let cap: i64 = MAX_BELIEF_TRAJECTORY;
    binds.push(&cap);
    stmt.query_map(rusqlite::params_from_iter(binds), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, f64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })
    .am("sync feed belief entries query")?
    .collect::<Result<Vec<_>, _>>()
    .am("sync feed belief entries map")
}

/// SQLite body of `SyncStore::feed_beliefs`: opened propositions, contexts,
/// and full trajectories with each row's change stamp.
pub(crate) fn feed_beliefs_sqlite(
    store: &SqliteStore,
    bank_id: &str,
    since: &str,
    limit: usize,
) -> Result<Vec<(String, BeliefObject)>, AmError> {
    let (since_stamp, since_entity) = split_push_watermark(since);
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(FEED_BELIEFS_SQL)
            .am("sync feed beliefs prepare")?;
        let rows = stmt
            .query_map(
                params![bank_id, since_stamp, since_entity, limit as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                    ))
                },
            )
            .am("sync feed beliefs query")?
            .collect::<Result<Vec<_>, _>>()
            .am("sync feed beliefs map")?;
        // Trajectories read on the same connection: `with_conn` is not
        // re-entrant, so nested store calls would deadlock here. The load is
        // bounded to this page's belief ids AND a per-belief cap, so one feed
        // page can never pull an unbounded number of confidence entries.
        let ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
        let mapped = load_belief_trajectories(conn, &ids)?;
        let mut out = Vec::new();
        for (
            id,
            _,
            subject_id,
            proposition_cipher,
            context_cipher,
            context_as_of,
            created_at,
            stamp,
        ) in rows
        {
            let history = mapped
                .iter()
                .filter(|(belief_id, ..)| belief_id == &id)
                .map(|(_, as_of, confidence, evidence_ref, source)| {
                    Ok(ConfidenceEntry {
                        as_of: as_of.clone(),
                        confidence: *confidence,
                        evidence_ref: evidence_ref.clone(),
                        source_type: clio_types::SourceType::parse(source)?,
                    })
                })
                .collect::<Result<Vec<_>, AmError>>()
                .am("sync feed belief entries parse")?;
            let proposition =
                crate::belief_store::open_proposition(store.kms().as_ref(), &proposition_cipher)?;
            let updated_at = history
                .last()
                .map(|entry| entry.as_of.clone())
                .unwrap_or_else(|| created_at.clone());
            out.push((
                stamp,
                BeliefObject {
                    id,
                    bank_id: bank_id.to_owned(),
                    subject_id,
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
