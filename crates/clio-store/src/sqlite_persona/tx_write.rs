// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite persona write transaction
//! bodies: the stable close-open + encrypted insert and the preference
//! upsert, including the context-preservation rule.
//!
//! ## Owns
//! - SQLite `put_stable_entry_tx` / `put_preference_state_tx` SQL
//! - preserve-on-omit: a stable version written without a caller context
//!   carries the prior open version's sealed context into the new version
//! - The erased-bank fail-closed guard for persona writes: both single-op and
//!   batch write transactions probe `persona_erase_tombstones` first and
//!   refuse with `ErasedSubject` (no persona context resurrection)
//! - Prior-context read helpers shared with the single-op audit emission
//!
//! ## Does not own
//! - The `PersonaStore` contract or sealing helpers (`persona_store`)
//! - Single-op audit row emission (`sqlite_persona`)
//! - Postgres parity (see `postgres_persona::tx_write`)
//!
//! ## Boundary
//!
//! Changes to the SQLite persona write SQL should normally require changes here.
//! Changes to audit emission should require `sqlite_persona`.
//!
//! Keep this module focused on SQLite persona write transaction bodies.

use crate::dek::DekProvider;
use crate::migrate::{IntoAmError, now_iso};
use crate::persona_store::{
    map_persona_insert, open_persona_context, persona_audit_detail, persona_context_action,
    unique_persona_id,
};
use clio_types::{AmError, ErrorCode, PersonaPreference, PersonaStableEntry};
use rusqlite::OptionalExtension;

use super::write_persona_audit;

/// Refuse a persona write into a bank whose persona context a verified
/// erasure purged: the erasure writes a bank-level `persona_erase_tombstones`
/// row (persona rows carry no subject attribution, so the purge is
/// bank-scoped) and every persona write transaction probes it first. Fail
/// closed with `ErasedSubject` — no persona context resurrection.
fn check_persona_erase_tombstone(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
) -> Result<(), AmError> {
    let hit: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM persona_erase_tombstones WHERE bank_id = ?1",
            rusqlite::params![bank_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| crate::migrate::map_db_err("persona erase tombstone probe", e))?;
    if hit.is_some() {
        return Err(AmError::new(
            ErrorCode::ErasedSubject,
            "erased persona context resurrection",
        ));
    }
    Ok(())
}

/// Prior open version's sealed `context_cipher` for one `(bank, key)`: `None`
/// when the key has no open version or that version was written without a
/// context. Read BEFORE the close-open (the lookup matches open rows only).
pub(crate) fn prior_open_stable_context(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    key: &str,
) -> Result<Option<String>, AmError> {
    tx.query_row(
        "SELECT context_cipher FROM persona_stable_entries
         WHERE bank_id = ?1 AND attr_key = ?2 AND valid_until IS NULL",
        rusqlite::params![bank_id, key],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| crate::migrate::map_db_err("persona prior context", e))
    .map(|row: Option<Option<String>>| row.flatten())
}

/// Existing preference row's sealed `context_cipher` (None when the row is
/// new or was written without a context). Feeds the audit detail only; the
/// upsert's `COALESCE` handles the preservation itself.
pub(crate) fn stored_preference_context(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    key: &str,
) -> Result<Option<String>, AmError> {
    tx.query_row(
        "SELECT context_cipher FROM persona_preferences
         WHERE bank_id = ?1 AND attr_key = ?2",
        rusqlite::params![bank_id, key],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| crate::migrate::map_db_err("persona stored context", e))
    .map(|row: Option<Option<String>>| row.flatten())
}

fn close_open_stable(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    key: &str,
    valid_until: &str,
    tx_until: &str,
) -> Result<(), AmError> {
    tx.execute(
        "UPDATE persona_stable_entries
         SET valid_until = ?3, tx_until = ?4, updated_at = ?4
         WHERE bank_id = ?1 AND attr_key = ?2 AND valid_until IS NULL",
        rusqlite::params![bank_id, key, valid_until, tx_until],
    )
    .am("persona stable close prior")?;
    Ok(())
}

/// Transaction-scoped stable-entry write: subject_keys upsert + close-open +
/// encrypted row insert. Shared by the single-op path and the batch executor.
///
/// `context_cipher` is the caller-sealed optional context (batch callers seal
/// via `seal_persona_context`). Ordinary updates preserve context: when
/// the caller omits one (`None`), the prior open version's sealed context is
/// carried into the new version; a fresh key with no prior version stays
/// without a context. A supplied context is the explicit replacement act.
pub(crate) fn put_stable_entry_tx(
    tx: &rusqlite::Transaction<'_>,
    entry: &PersonaStableEntry,
    cipher: &str,
    dek_ref: &str,
    context_cipher: Option<&str>,
) -> Result<(), AmError> {
    let id = unique_persona_id("pse", &entry.bank_id, &entry.key);
    let ts = now_iso();
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
            rusqlite::params![entry.bank_id, dek_ref, ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows as u64, &entry.bank_id)?;
    let prior = prior_open_stable_context(tx, &entry.bank_id, &entry.key)?;
    let context_cipher = context_cipher.or(prior.as_deref());
    close_open_stable(tx, &entry.bank_id, &entry.key, &entry.as_of, &ts)?;
    map_persona_insert(
        tx.execute(
            "INSERT INTO persona_stable_entries (
                id, bank_id, attr_key, value_cipher, context_cipher, confidence,
                source, source_ref, admission_score, valid_from, valid_until,
                tx_from, tx_until, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, NULL, ?11, ?11)",
            rusqlite::params![
                id,
                entry.bank_id,
                entry.key,
                cipher,
                context_cipher,
                entry.confidence,
                entry.source,
                entry.source_ref,
                entry.admission_score,
                entry.as_of,
                ts,
            ],
        )
        .map(|n| n as u64)
        .map_err(|e| e.to_string()),
    )?;
    Ok(())
}

/// Transaction-scoped preference upsert (plaintext numeric columns; no KMS
/// needed). `context_cipher` is the caller-sealed optional context: `Some`
/// replaces the row's stored context (the explicit audited act), `None`
/// preserves it on conflict (`COALESCE(excluded, stored)` ordinary
/// updates never clear it); batch callers seal via `seal_persona_context`.
///
/// `context_as_of` tracks when that stored context became effective: a write
/// that supplies a context sets it to this write's `as_of`; an ordinary update
/// keeps the stored `context_as_of` alongside the preserved context.
pub(crate) fn put_preference_state_tx(
    tx: &rusqlite::Transaction<'_>,
    pref: &PersonaPreference,
    context_cipher: Option<&str>,
) -> Result<(), AmError> {
    let id = unique_persona_id("ppf", &pref.bank_id, &pref.key);
    let ts = now_iso();
    // A supplied context takes effect at this write's as_of; an omitted context
    // carries no new effective time (the CASE below preserves the stored one).
    let context_as_of = context_cipher.map(|_| pref.as_of.as_str());
    tx.execute(
        "INSERT INTO persona_preferences (
            id, bank_id, attr_key, value, trend, confidence, source, source_ref,
            admission_score, as_of, context_cipher, context_as_of, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
         ON CONFLICT(bank_id, attr_key) DO UPDATE SET
           value = excluded.value,
           trend = excluded.trend,
           confidence = excluded.confidence,
           source = excluded.source,
           source_ref = excluded.source_ref,
           as_of = excluded.as_of,
           context_cipher = COALESCE(excluded.context_cipher, persona_preferences.context_cipher),
           context_as_of = CASE WHEN excluded.context_cipher IS NOT NULL
                                THEN excluded.as_of
                                ELSE persona_preferences.context_as_of END,
           updated_at = excluded.updated_at",
        rusqlite::params![
            id,
            pref.bank_id,
            pref.key,
            pref.value,
            pref.trend,
            pref.confidence,
            pref.source,
            pref.source_ref,
            pref.admission_score,
            pref.as_of,
            context_cipher,
            context_as_of,
            ts,
        ],
    )
    .am("persona preference upsert")?;
    Ok(())
}

/// Transaction-scoped stable write plus its audit row: shared by the
/// single-op path and the batch executor so every persona stable write emits
/// the same telemetry (presence/length/hash, never raw text).
///
/// This is the sole persona stable write entry point, so the erased-bank
/// guard runs here first: a bank-level persona erase tombstone makes
/// the write fail closed with `ErasedSubject`. SQLite needs no extra lock —
/// a single writer serializes this probe against the erase transaction.
pub(crate) fn put_stable_entry_tx_audited(
    tx: &rusqlite::Transaction<'_>,
    kms: &dyn DekProvider,
    entry: &PersonaStableEntry,
    cipher: &str,
    dek_ref: &str,
    context_cipher: Option<&str>,
    actor: &str,
) -> Result<(), AmError> {
    check_persona_erase_tombstone(tx, &entry.bank_id)?;
    // Prior open version's context, read before the write helper closes it:
    // it feeds the audit stats, and the closed prior version row retains its
    // own ciphertext (the prior stays reconstructable through stable_history).
    let prior_cipher = prior_open_stable_context(tx, &entry.bank_id, &entry.key)?;
    let prior_context = open_persona_context(kms, prior_cipher.as_deref());
    put_stable_entry_tx(tx, entry, cipher, dek_ref, context_cipher)?;
    let detail = persona_audit_detail(
        "stable",
        &entry.key,
        persona_context_action(entry.context.is_some(), prior_context.is_some()),
        prior_context.as_deref(),
        entry.context.as_deref().or(prior_context.as_deref()),
    );
    write_persona_audit(
        tx,
        &entry.bank_id,
        "persona_put_stable",
        &format!("persona-stable:{}", entry.key),
        &entry.key,
        actor,
        &detail,
    )
}

/// Transaction-scoped preference upsert plus its audit row: shared by
/// the single-op path and the batch executor. The erased-bank guard runs
/// first: a bank-level persona erase tombstone makes the write fail
/// closed with `ErasedSubject`.
pub(crate) fn put_preference_state_tx_audited(
    tx: &rusqlite::Transaction<'_>,
    kms: &dyn DekProvider,
    pref: &PersonaPreference,
    context_cipher: Option<&str>,
    actor: &str,
) -> Result<(), AmError> {
    check_persona_erase_tombstone(tx, &pref.bank_id)?;
    // The existing row's sealed context feeds the audit stats; a supplied
    // context is the explicit replacement act. The prior ciphertext is NOT
    // copied into the audit detail: audit_events is append-only, so a sealed
    // copy there would outlive subject erasure.
    let prior_cipher = stored_preference_context(tx, &pref.bank_id, &pref.key)?;
    let prior_context = open_persona_context(kms, prior_cipher.as_deref());
    put_preference_state_tx(tx, pref, context_cipher)?;
    let supplied = pref.context.is_some();
    let detail = persona_audit_detail(
        "preference",
        &pref.key,
        persona_context_action(supplied, prior_context.is_some()),
        prior_context.as_deref(),
        pref.context.as_deref().or(prior_context.as_deref()),
    );
    write_persona_audit(
        tx,
        &pref.bank_id,
        "persona_put_preference",
        &format!("persona-preference:{}", pref.key),
        &pref.key,
        actor,
        &detail,
    )
}
