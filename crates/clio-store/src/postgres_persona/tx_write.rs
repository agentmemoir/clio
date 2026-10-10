// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres persona write transaction
//! bodies: the stable close-open + encrypted insert and the preference
//! upsert, including the context-preservation rule.
//!
//! ## Owns
//! - Postgres `put_stable_entry_tx` / `put_preference_state_postgres_tx` SQL
//! - preserve-on-omit: a stable version written without a caller context
//!   carries the prior open version's sealed context into the new version
//! - The erased-bank fail-closed guard for persona writes: both single-op and
//!   batch write transactions take the bank's `persona-erase` advisory lock,
//!   probe `persona_erase_tombstones`, and refuse with `ErasedSubject`
//! - The `persona-write:<bank>:<key>` advisory lock that serializes
//!   concurrent writes to one `(bank, key)` slot (audit predecessor safety)
//! - Prior-context read helpers shared with the single-op audit emission
//!
//! ## Does not own
//! - The `PersonaStore` contract or sealing helpers (`persona_store`)
//! - Single-op audit row emission (`postgres_persona`)
//! - SQLite parity (see `sqlite_persona::tx_write`)
//!
//! ## Boundary
//!
//! Changes to the Postgres persona write SQL should normally require changes
//! here. Changes to audit emission should require `postgres_persona`.
//!
//! Keep this module focused on Postgres persona write transaction bodies.

use crate::dek::DekProvider;
use crate::migrate::{IntoAmError, now_iso};
use crate::persona_store::{
    map_persona_insert, open_persona_context, persona_audit_detail, persona_context_action,
    unique_persona_id,
};
use clio_types::{AmError, ErrorCode, PersonaPreference, PersonaStableEntry};
use postgres::Transaction;

use super::write_persona_audit;

/// Refuse a persona write into a bank whose persona context a verified
/// erasure purged: the erasure writes a bank-level `persona_erase_tombstones`
/// row (persona rows carry no subject attribution, so the purge is
/// bank-scoped) and every persona write transaction probes it first. The
/// probe runs under the bank's `persona-erase` advisory lock, the same lock
/// the erase transaction takes before purging and tombstoning the bank, so
/// an in-flight persona write commits before the erase runs and a write that
/// starts after the erase observes the tombstone. Fail closed with
/// `ErasedSubject` — no persona context resurrection.
fn check_persona_erase_tombstone(tx: &mut Transaction<'_>, bank_id: &str) -> Result<(), AmError> {
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext('persona-erase:' || $1))",
        &[&bank_id],
    )
    .am("persona erase advisory lock")?;
    let hit = tx
        .query_opt(
            "SELECT 1 FROM persona_erase_tombstones WHERE bank_id = $1",
            &[&bank_id],
        )
        .am("persona erase tombstone probe")?;
    if hit.is_some() {
        return Err(AmError::new(
            ErrorCode::ErasedSubject,
            "erased persona context resurrection",
        ));
    }
    Ok(())
}

/// Serialize concurrent persona writes to one `(bank, key)` slot
/// (`persona-write:<bank>:<key>`, transaction-scoped): two first writes to
/// the same slot cannot interleave their locked predecessor reads and both
/// record a wrong audit predecessor.
fn lock_persona_write_slot(
    tx: &mut Transaction<'_>,
    bank_id: &str,
    key: &str,
) -> Result<(), AmError> {
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext('persona-write:' || $1 || ':' || $2))",
        &[&bank_id, &key],
    )
    .am("persona write advisory lock")?;
    Ok(())
}

/// Prior open version's sealed `context_cipher` for one `(bank, key)`: `None`
/// when the key has no open version or that version was written without a
/// context. Read BEFORE the close-open (the lookup matches open rows only).
///
/// `FOR UPDATE` locks the open row this transaction is about to close; a
/// missing row locks nothing, which is why the `persona-write` advisory lock
/// in the audited write entry points serializes the missing-row case.
pub(crate) fn prior_open_stable_context(
    tx: &mut Transaction<'_>,
    bank_id: &str,
    key: &str,
) -> Result<Option<String>, AmError> {
    let row: Option<Option<String>> = tx
        .query_opt(
            "SELECT context_cipher FROM persona_stable_entries
             WHERE bank_id = $1 AND attr_key = $2 AND valid_until IS NULL
             FOR UPDATE",
            &[&bank_id, &key],
        )
        .am("persona prior context")?
        .map(|r| r.get(0));
    Ok(row.flatten())
}

/// Existing preference row's sealed `context_cipher` (None when the row is
/// new or was written without a context). Feeds the audit detail only; the
/// upsert's `COALESCE` handles the preservation itself.
///
/// `FOR UPDATE` locks the row this transaction is about to replace so two
/// concurrent replacements cannot both record the same predecessor in their
/// audit detail. A missing row locks nothing.
pub(crate) fn stored_preference_context(
    tx: &mut Transaction<'_>,
    bank_id: &str,
    key: &str,
) -> Result<Option<String>, AmError> {
    let row: Option<Option<String>> = tx
        .query_opt(
            "SELECT context_cipher FROM persona_preferences
             WHERE bank_id = $1 AND attr_key = $2
             FOR UPDATE",
            &[&bank_id, &key],
        )
        .am("persona stored context")?
        .map(|r| r.get(0));
    Ok(row.flatten())
}

fn close_open_stable(
    tx: &mut Transaction<'_>,
    bank_id: &str,
    key: &str,
    valid_until: &str,
    tx_until: &str,
) -> Result<(), AmError> {
    tx.execute(
        "UPDATE persona_stable_entries
         SET valid_until = $3, tx_until = $4, updated_at = $4
         WHERE bank_id = $1 AND attr_key = $2 AND valid_until IS NULL",
        &[&bank_id, &key, &valid_until, &tx_until],
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
    tx: &mut Transaction<'_>,
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
         VALUES ($1, $2, 'active', $3, $3)
         ON CONFLICT (subject_id) DO UPDATE SET
           dek_ref = EXCLUDED.dek_ref,
           status = 'active',
           shredded_at = NULL,
           updated_at = EXCLUDED.updated_at
           WHERE subject_keys.shredded_at IS NULL",
            &[&entry.bank_id, &dek_ref, &ts],
        )
        .am("subject_keys upsert")?;
    crate::item_persist::check_subject_reactivation(sk_rows, &entry.bank_id)?;
    let prior = prior_open_stable_context(tx, &entry.bank_id, &entry.key)?;
    let context_cipher = context_cipher.or(prior.as_deref());
    close_open_stable(tx, &entry.bank_id, &entry.key, &entry.as_of, &ts)?;
    map_persona_insert(
        tx.execute(
            "INSERT INTO persona_stable_entries (
                id, bank_id, attr_key, value_cipher, context_cipher, confidence,
                source, source_ref, admission_score, valid_from, valid_until,
                tx_from, tx_until, created_at, updated_at
             ) VALUES ($1, $2, $3, $4, $5, $6::float8, $7, $8, $9::float8, $10, NULL, $11, NULL, $11, $11)",
            &[
                &id,
                &entry.bank_id,
                &entry.key,
                &cipher,
                &context_cipher,
                &entry.confidence,
                &entry.source,
                &entry.source_ref,
                &entry.admission_score,
                &entry.as_of,
                &ts,
            ],
        )
        .map_err(|e| e.to_string()),
    )?;
    Ok(())
}

/// Transaction-scoped preference upsert (plaintext numeric columns; no KMS
/// needed). `context_cipher` is the caller-sealed optional context: `Some`
/// replaces the row's stored context (the explicit audited act), `None`
/// preserves it on conflict (`COALESCE(EXCLUDED, stored)` ordinary
/// updates never clear it); batch callers seal via `seal_persona_context`.
///
/// `context_as_of` tracks when that stored context became effective: a write
/// that supplies a context sets it to this write's `as_of`; an ordinary update
/// keeps the stored `context_as_of` alongside the preserved context.
pub(crate) fn put_preference_state_postgres_tx(
    tx: &mut Transaction<'_>,
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
         ) VALUES ($1, $2, $3, $4::float8, $5::float8, $6::float8, $7, $8, $9::float8, $10, $11, $12, $13, $13)
         ON CONFLICT (bank_id, attr_key) DO UPDATE SET
           value = EXCLUDED.value,
           trend = EXCLUDED.trend,
           confidence = EXCLUDED.confidence,
           source = EXCLUDED.source,
           source_ref = EXCLUDED.source_ref,
           as_of = EXCLUDED.as_of,
           context_cipher = COALESCE(EXCLUDED.context_cipher, persona_preferences.context_cipher),
           context_as_of = CASE WHEN EXCLUDED.context_cipher IS NOT NULL
                                THEN EXCLUDED.as_of
                                ELSE persona_preferences.context_as_of END,
           updated_at = EXCLUDED.updated_at",
        &[
            &id,
            &pref.bank_id,
            &pref.key,
            &pref.value,
            &pref.trend,
            &pref.confidence,
            &pref.source,
            &pref.source_ref,
            &pref.admission_score,
            &pref.as_of,
            &context_cipher,
            &context_as_of,
            &ts,
        ],
    )
    .am("persona preference upsert")?;
    Ok(())
}

/// Transaction-scoped stable write plus its audit row: shared by the
/// single-op path and the batch executor so every persona stable write emits
/// the same telemetry (presence/length/hash, never raw text).
///
/// This is the sole persona stable write entry point, so the guards run at
/// the very start of the write transaction: first the bank's `persona-erase`
/// advisory lock plus tombstone probe (fail closed with `ErasedSubject`,
/// Then the `persona-write` slot lock so a concurrent first write to
/// the same `(bank, key)` serializes before the locked predecessor read.
pub(crate) fn put_stable_entry_tx_audited(
    tx: &mut Transaction<'_>,
    kms: &dyn DekProvider,
    entry: &PersonaStableEntry,
    cipher: &str,
    dek_ref: &str,
    context_cipher: Option<&str>,
    actor: &str,
) -> Result<(), AmError> {
    check_persona_erase_tombstone(tx, &entry.bank_id)?;
    lock_persona_write_slot(tx, &entry.bank_id, &entry.key)?;
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
/// the single-op path and the batch executor. The guards run at the very
/// start of the write transaction: the bank's `persona-erase` advisory lock
/// plus tombstone probe (fail closed with `ErasedSubject`), then the
/// `persona-write` slot lock before the locked predecessor read.
pub(crate) fn put_preference_state_tx_audited(
    tx: &mut Transaction<'_>,
    kms: &dyn DekProvider,
    pref: &PersonaPreference,
    context_cipher: Option<&str>,
    actor: &str,
) -> Result<(), AmError> {
    check_persona_erase_tombstone(tx, &pref.bank_id)?;
    lock_persona_write_slot(tx, &pref.bank_id, &pref.key)?;
    // The existing row's sealed context feeds the audit stats; a supplied
    // context is the explicit replacement act. The prior ciphertext is NOT
    // copied into the audit detail: audit_events is append-only, so a sealed
    // copy there would outlive subject erasure.
    let prior_cipher = stored_preference_context(tx, &pref.bank_id, &pref.key)?;
    let prior_context = open_persona_context(kms, prior_cipher.as_deref());
    put_preference_state_postgres_tx(tx, pref, context_cipher)?;
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
