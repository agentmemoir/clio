// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres persona-document repository.
//!
//! ## Owns
//! - Postgres stable put / preference upsert / document read / history SQL (the
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
//! Changes to Postgres persona SQL should normally require changes here.
//! Changes to SQLite persona SQL should not require changes here.
//!
//! Keep this module focused on Postgres persona SQL.

use crate::item_persist::require_actor;
use crate::migrate::{IntoAmError, now_iso};
use crate::persona_store::{
    PersonaStore, open_persona_context, open_persona_value, persona_require_nonempty,
    seal_persona_context, seal_persona_value, unique_persona_id,
};
use crate::postgres::PostgresStore;
use clio_types::{AmError, PersonaDocument, PersonaPreference, PersonaStableEntry};
use postgres::Transaction;

mod tx_write;

pub(crate) use tx_write::{put_preference_state_tx_audited, put_stable_entry_tx_audited};

/// Audit row for one single-op persona write (telemetry: the
/// detail records context presence/length/hash for prior and effective
/// values, never the raw text). `item_id` is the per-key persona handle
/// (`persona-stable:<key>` / `persona-preference:<key>`) so the write
/// lifecycle groups by record; persona rows are not ledger items, so these
/// rows are reachable through `audit_events` reads, not item audit trails.
pub(crate) fn write_persona_audit(
    tx: &mut Transaction<'_>,
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
         ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        &[
            &unique_persona_id("paudit", bank_id, key),
            &bank_id,
            &operation,
            &handle,
            &actor,
            &detail,
            &now_iso(),
        ],
    )
    .am("persona audit insert")?;
    Ok(())
}

impl PostgresStore {
    fn put_stable_entry_impl(
        &self,
        entry: &PersonaStableEntry,
        actor: &str,
    ) -> Result<(), AmError> {
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
        self.with_client(|client| {
            let mut tx = client.transaction().am("persona stable begin")?;
            // The shared tx helper emits the audit row with the same
            // stats the batch executor records.
            put_stable_entry_tx_audited(
                &mut tx,
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

    fn put_preference_state_impl(
        &self,
        pref: &PersonaPreference,
        actor: &str,
    ) -> Result<(), AmError> {
        pref.validate()?;
        require_actor(actor)?;
        let context_cipher =
            seal_persona_context(self.kms().as_ref(), &pref.bank_id, pref.context.as_deref())?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("persona preference begin")?;
            // The shared tx helper emits the audit row (a supplied
            // context is the explicit replacement act; the detail keeps
            // presence/length/hash statistics only and never retains the
            // prior sealed ciphertext).
            put_preference_state_tx_audited(
                &mut tx,
                self.kms().as_ref(),
                pref,
                context_cipher.as_deref(),
                actor,
            )?;
            tx.commit().am("persona preference commit")?;
            Ok(())
        })
    }

    fn get_persona_document_impl(&self, bank_id: &str) -> Result<PersonaDocument, AmError> {
        persona_require_nonempty(bank_id, "bank_id")?;
        let stable = self.open_stable_rows(bank_id)?;
        let preferences = self.preference_rows(bank_id)?;
        Ok(PersonaDocument {
            stable,
            preferences,
        })
    }

    fn get_preference_impl(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<PersonaPreference>, AmError> {
        persona_require_nonempty(bank_id, "bank_id")?;
        persona_require_nonempty(key, "key")?;
        Ok(self
            .with_client(|client| {
                client
                    .query_opt(
                        "SELECT attr_key, value::float8, trend::float8, confidence::float8,
                                source, source_ref, admission_score::float8, as_of, context_cipher
                         FROM persona_preferences
                         WHERE bank_id = $1 AND attr_key = $2",
                        &[&bank_id, &key],
                    )
                    .am("persona preference read one")
            })?
            .map(|r| {
                let context_cipher: Option<String> = r.get(8);
                PersonaPreference {
                    bank_id: bank_id.to_owned(),
                    key: r.get(0),
                    value: r.get(1),
                    trend: r.get(2),
                    confidence: r.get(3),
                    source: r.get(4),
                    source_ref: r.get(5),
                    admission_score: r.get(6),
                    as_of: r.get(7),
                    context: open_persona_context(self.kms().as_ref(), context_cipher.as_deref()),
                }
            }))
    }

    fn stable_history_impl(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Vec<PersonaStableEntry>, AmError> {
        persona_require_nonempty(bank_id, "bank_id")?;
        persona_require_nonempty(key, "key")?;
        let rows = self.with_client(|client| {
            client
                .query(
                    "SELECT value_cipher, context_cipher, confidence::float8, source, source_ref,
                            admission_score::float8, valid_from, valid_until
                     FROM persona_stable_entries
                     WHERE bank_id = $1 AND attr_key = $2
                     ORDER BY valid_from, id",
                    &[&bank_id, &key],
                )
                .am("persona history query")
        })?;
        rows.into_iter()
            .map(|r| -> Result<PersonaStableEntry, AmError> {
                let cipher: String = r.get(0);
                let context_cipher: Option<String> = r.get(1);
                Ok(PersonaStableEntry {
                    bank_id: bank_id.to_owned(),
                    key: key.to_owned(),
                    value: open_persona_value(self.kms().as_ref(), &cipher)?,
                    confidence: r.get(2),
                    source: r.get(3),
                    source_ref: r.get(4),
                    context: open_persona_context(self.kms().as_ref(), context_cipher.as_deref()),
                    admission_score: r.get(5),
                    as_of: r.get(6),
                    valid_until: r.get(7),
                })
            })
            .collect()
    }

    fn inspect_stable_ciphertext_impl(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<String>, AmError> {
        Ok(self
            .with_client(|client| {
                client
                    .query_opt(
                        "SELECT value_cipher FROM persona_stable_entries
                         WHERE bank_id = $1 AND attr_key = $2 AND valid_until IS NULL",
                        &[&bank_id, &key],
                    )
                    .am("persona ciphertext inspect")
            })?
            .map(|r| r.get(0)))
    }

    fn open_stable_rows(&self, bank_id: &str) -> Result<Vec<PersonaStableEntry>, AmError> {
        let rows = self.with_client(|client| {
            client
                .query(
                    "SELECT attr_key, value_cipher, context_cipher, confidence::float8, source,
                            source_ref, admission_score::float8, valid_from
                     FROM persona_stable_entries
                     WHERE bank_id = $1 AND valid_until IS NULL
                     ORDER BY attr_key",
                    &[&bank_id],
                )
                .am("persona stable read")
        })?;
        rows.into_iter()
            .map(|r| -> Result<PersonaStableEntry, AmError> {
                let cipher: String = r.get(1);
                let context_cipher: Option<String> = r.get(2);
                Ok(PersonaStableEntry {
                    bank_id: bank_id.to_owned(),
                    key: r.get(0),
                    value: open_persona_value(self.kms().as_ref(), &cipher)?,
                    confidence: r.get(3),
                    source: r.get(4),
                    source_ref: r.get(5),
                    context: open_persona_context(self.kms().as_ref(), context_cipher.as_deref()),
                    admission_score: r.get(6),
                    as_of: r.get(7),
                    valid_until: None,
                })
            })
            .collect()
    }

    fn preference_rows(&self, bank_id: &str) -> Result<Vec<PersonaPreference>, AmError> {
        let rows = self.with_client(|client| {
            client
                .query(
                    "SELECT attr_key, value::float8, trend::float8, confidence::float8, source,
                            source_ref, admission_score::float8, as_of, context_cipher
                     FROM persona_preferences
                     WHERE bank_id = $1
                     ORDER BY attr_key",
                    &[&bank_id],
                )
                .am("persona preference read")
        })?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let context_cipher: Option<String> = r.get(8);
                PersonaPreference {
                    bank_id: bank_id.to_owned(),
                    key: r.get(0),
                    value: r.get(1),
                    trend: r.get(2),
                    confidence: r.get(3),
                    source: r.get(4),
                    source_ref: r.get(5),
                    admission_score: r.get(6),
                    as_of: r.get(7),
                    context: open_persona_context(self.kms().as_ref(), context_cipher.as_deref()),
                }
            })
            .collect())
    }
}

impl PersonaStore for PostgresStore {
    fn put_stable_entry(&self, entry: &PersonaStableEntry, actor: &str) -> Result<(), AmError> {
        self.put_stable_entry_impl(entry, actor)
    }

    fn put_preference_state(&self, pref: &PersonaPreference, actor: &str) -> Result<(), AmError> {
        self.put_preference_state_impl(pref, actor)
    }

    fn get_persona_document(&self, bank_id: &str) -> Result<PersonaDocument, AmError> {
        self.get_persona_document_impl(bank_id)
    }

    fn get_preference(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<PersonaPreference>, AmError> {
        self.get_preference_impl(bank_id, key)
    }

    fn stable_history(&self, bank_id: &str, key: &str) -> Result<Vec<PersonaStableEntry>, AmError> {
        self.stable_history_impl(bank_id, key)
    }

    fn inspect_stable_ciphertext(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<String>, AmError> {
        self.inspect_stable_ciphertext_impl(bank_id, key)
    }
}

#[cfg(test)]
#[path = "postgres_persona_tests.rs"]
mod postgres_persona_tests;
