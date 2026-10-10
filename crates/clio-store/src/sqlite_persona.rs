// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite persona-document repository.
//!
//! ## Owns
//! - SQLite stable put / preference upsert / document read / history SQL (the
//!   write transaction bodies live in `tx_write`)
//! - The optional sealed `context_cipher` on both persona tables (write,
//!   preserve-on-omit upsert, tolerant open)
//! - Single-op persona write audit rows (`audit_events`)
//! - The erased-bank fail-closed guard for persona writes (a bank-level
//!   persona erase tombstone makes both put paths return `ErasedSubject`)
//!
//! ## Does not own
//! - The `PersonaStore` contract (see `persona_store`)
//! - Admission scoring or tool orchestration (`clio-persona`)
//!
//! ## Boundary
//!
//! Changes to SQLite persona SQL should normally require changes here.
//! Changes to Postgres persona SQL should not require changes here.
//!
//! Keep this module focused on SQLite persona SQL.

use crate::dek::DekProvider;
use crate::item_persist::require_actor;
use crate::migrate::{IntoAmError, now_iso};
use crate::persona_store::{
    PersonaStore, open_persona_context, open_persona_value, persona_require_nonempty,
    seal_persona_context, seal_persona_value, unique_persona_id,
};
use crate::sqlite::SqliteStore;
pub(crate) use crate::sqlite_persona::tx_write::{
    put_preference_state_tx_audited, put_stable_entry_tx_audited,
};
use clio_types::{AmError, PersonaDocument, PersonaPreference, PersonaStableEntry};
use rusqlite::{OptionalExtension, params};

mod tx_write;

/// Audit row for one single-op persona write (telemetry: the
/// detail records context presence/length/hash for prior and effective
/// values, never the raw text). `item_id` is the per-key persona handle
/// (`persona-stable:<key>` / `persona-preference:<key>`) so the write
/// lifecycle groups by record; persona rows are not ledger items, so these
/// rows are reachable through `audit_events` reads, not item audit trails.
pub(crate) fn write_persona_audit(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    operation: &str,
    handle: &str,
    key: &str,
    actor: &str,
    detail: &str,
) -> Result<(), AmError> {
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, actor, detail_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            unique_persona_id("paudit", bank_id, key),
            bank_id,
            operation,
            handle,
            actor,
            detail,
            now_iso(),
        ],
    )
    .am("persona audit insert")?;
    Ok(())
}

impl PersonaStore for SqliteStore {
    fn put_stable_entry(&self, entry: &PersonaStableEntry, actor: &str) -> Result<(), AmError> {
        entry.validate()?;
        require_actor(actor)?;
        persona_require_nonempty(&entry.bank_id, "bank_id")?;
        let dek_ref = self.kms().ensure_dek(&entry.bank_id)?;
        let cipher = seal_persona_value(self.kms().as_ref(), &entry.bank_id, &entry.value)?;
        let context_cipher = seal_persona_context(
            self.kms().as_ref(),
            &entry.bank_id,
            entry.context.as_deref(),
        )?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("persona stable begin")?;
            // The shared tx helper emits the audit row with the same
            // stats the batch executor records.
            put_stable_entry_tx_audited(
                &tx,
                self.kms().as_ref(),
                entry,
                &cipher,
                &dek_ref,
                context_cipher.as_deref(),
                actor,
            )?;
            tx.commit().am("persona stable commit")?;
            Ok(())
        })
    }

    fn put_preference_state(&self, pref: &PersonaPreference, actor: &str) -> Result<(), AmError> {
        pref.validate()?;
        require_actor(actor)?;
        let context_cipher =
            seal_persona_context(self.kms().as_ref(), &pref.bank_id, pref.context.as_deref())?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("persona preference begin")?;
            // The shared tx helper emits the audit row (a supplied
            // context is the explicit replacement act; the detail keeps
            // presence/length/hash statistics only and never retains the
            // prior sealed ciphertext).
            put_preference_state_tx_audited(
                &tx,
                self.kms().as_ref(),
                pref,
                context_cipher.as_deref(),
                actor,
            )?;
            tx.commit().am("persona preference commit")?;
            Ok(())
        })
    }

    fn get_persona_document(&self, bank_id: &str) -> Result<PersonaDocument, AmError> {
        persona_require_nonempty(bank_id, "bank_id")?;
        let stable_rows = self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT attr_key, value_cipher, context_cipher, confidence, source,
                            source_ref, admission_score, valid_from
                     FROM persona_stable_entries
                     WHERE bank_id = ?1 AND valid_until IS NULL
                     ORDER BY attr_key",
                )
                .am("persona stable read prepare")?;
            let rows = stmt
                .query_map(params![bank_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<f64>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, f64>(6)?,
                        r.get::<_, String>(7)?,
                    ))
                })
                .am("persona stable read")?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| crate::migrate::map_db_err("persona stable rows", e))
        })?;
        let stable = stable_rows
            .into_iter()
            .map(
                |(
                    key,
                    cipher,
                    context_cipher,
                    confidence,
                    source,
                    source_ref,
                    admission_score,
                    as_of,
                )| {
                    Ok(PersonaStableEntry {
                        bank_id: bank_id.to_owned(),
                        key,
                        value: open_persona_value(self.kms().as_ref(), &cipher)?,
                        confidence,
                        source,
                        source_ref,
                        context: open_persona_context(
                            self.kms().as_ref(),
                            context_cipher.as_deref(),
                        ),
                        admission_score,
                        as_of,
                        valid_until: None,
                    })
                },
            )
            .collect::<Result<Vec<_>, _>>()?;
        let pref_rows = self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT attr_key, value, trend, confidence, source, source_ref,
                            admission_score, as_of, context_cipher
                     FROM persona_preferences
                     WHERE bank_id = ?1
                     ORDER BY attr_key",
                )
                .am("persona preference read prepare")?;
            let rows = stmt
                .query_map(params![bank_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, f64>(2)?,
                        r.get::<_, Option<f64>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, f64>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                })
                .am("persona preference read")?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| crate::migrate::map_db_err("persona preference rows", e))
        })?;
        let preferences = pref_rows_to_preferences(self.kms().as_ref(), bank_id, pref_rows);
        Ok(PersonaDocument {
            stable,
            preferences,
        })
    }

    fn get_preference(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<PersonaPreference>, AmError> {
        persona_require_nonempty(bank_id, "bank_id")?;
        persona_require_nonempty(key, "key")?;
        let row = self.with_conn(|conn| {
            conn.query_row(
                "SELECT attr_key, value, trend, confidence, source, source_ref,
                        admission_score, as_of, context_cipher
                 FROM persona_preferences
                 WHERE bank_id = ?1 AND attr_key = ?2",
                params![bank_id, key],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, f64>(2)?,
                        r.get::<_, Option<f64>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, f64>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()
            .am("persona preference read one")
        })?;
        Ok(row
            .map(|r| pref_rows_to_preferences(self.kms().as_ref(), bank_id, vec![r]))
            .and_then(|mut prefs| prefs.pop()))
    }

    fn stable_history(&self, bank_id: &str, key: &str) -> Result<Vec<PersonaStableEntry>, AmError> {
        persona_require_nonempty(bank_id, "bank_id")?;
        persona_require_nonempty(key, "key")?;
        let rows = self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT value_cipher, context_cipher, confidence, source, source_ref,
                            admission_score, valid_from, valid_until
                     FROM persona_stable_entries
                     WHERE bank_id = ?1 AND attr_key = ?2
                     ORDER BY valid_from, id",
                )
                .am("persona history prepare")?;
            let rows = stmt
                .query_map(params![bank_id, key], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<f64>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, f64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                    ))
                })
                .am("persona history query")?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| crate::migrate::map_db_err("persona history rows", e))
        })?;
        rows.into_iter()
            .map(
                |(
                    cipher,
                    context_cipher,
                    confidence,
                    source,
                    source_ref,
                    admission_score,
                    valid_from,
                    valid_until,
                )| {
                    Ok(PersonaStableEntry {
                        bank_id: bank_id.to_owned(),
                        key: key.to_owned(),
                        value: open_persona_value(self.kms().as_ref(), &cipher)?,
                        confidence,
                        source,
                        source_ref,
                        context: open_persona_context(
                            self.kms().as_ref(),
                            context_cipher.as_deref(),
                        ),
                        admission_score,
                        as_of: valid_from,
                        valid_until,
                    })
                },
            )
            .collect()
    }

    fn inspect_stable_ciphertext(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<String>, AmError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT value_cipher FROM persona_stable_entries
                 WHERE bank_id = ?1 AND attr_key = ?2 AND valid_until IS NULL",
                params![bank_id, key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .am("persona ciphertext inspect")
        })
    }
}

fn pref_rows_to_preferences(
    kms: &dyn DekProvider,
    bank_id: &str,
    rows: Vec<(
        String,
        f64,
        f64,
        Option<f64>,
        String,
        Option<String>,
        f64,
        String,
        Option<String>,
    )>,
) -> Vec<PersonaPreference> {
    rows.into_iter()
        .map(
            |(
                key,
                value,
                trend,
                confidence,
                source,
                source_ref,
                admission_score,
                as_of,
                context_cipher,
            )| {
                PersonaPreference {
                    bank_id: bank_id.to_owned(),
                    key,
                    value,
                    trend,
                    confidence,
                    source,
                    source_ref,
                    context: open_persona_context(kms, context_cipher.as_deref()),
                    admission_score,
                    as_of,
                }
            },
        )
        .collect()
}

#[cfg(test)]
#[path = "sqlite_persona_tests.rs"]
mod sqlite_persona_tests;
