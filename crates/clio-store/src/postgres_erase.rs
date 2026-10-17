// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres compliance-erasure transactions:
//! the derived-row purge and shred marker that commit first, the DEK
//! destruction that follows them, and the content-free tombstone insert.
//!
//! ## Owns
//! - `ComplianceStore` implementation for [`PostgresStore`]
//! - The Postgres erasure split: the purge and shred marker commit, then the
//!   DEK destruction, then the tombstones and audit row
//! - The bank-level persona erase tombstone and per-bank advisory lock taken
//!   for every bank whose persona context this erasure purges (fail-closes
//!   later persona writes)
//! - Cross-bank fan-out of the derived-structure purge, which commits before
//!   the DEK is destroyed
//!
//! ## Does not own
//! - Erasure authorization/confirmation or telemetry (`clio-compliance`)
//! - The derived-row purge SQL (`postgres_erase::derived_purge`) or the
//!   persona-context purge SQL (`postgres_erase::persona_purge`)
//! - SQLite parity (see `sqlite_erase`)
//!
//! ## Boundary
//!
//! Changes to the Postgres erasure transaction ordering should normally
//! require changes here. Changes to the compatibility store contract should
//! require `compliance_store`.
//!
//! Keep this module focused on the Postgres erasure transaction.

use crate::compliance_store::{
    BankScopeStore, ComplianceStore, ComplianceTombstoneView, SubjectEraseOutcome,
    SubjectEraseSpec, validate_spec,
};
use crate::erase_hash::sha256_hex;
use crate::migrate::IntoAmError;
use crate::model::ItemIdentity;
use crate::postgres::PostgresStore;
use crate::postgres_erase::derived_purge::{purge_bank, select_items, select_subject_banks};
use crate::postgres_erase::persona_purge::{
    bank_has_persona_context, lock_persona_erase, purge_persona_context,
    write_persona_erase_tombstone,
};
use crate::postgres_erase::tombstone::insert_tombstone;
use clio_types::{AmError, ErrorCode};

mod derived_purge;
mod persona_purge;
mod tombstone;
mod triple_purge;

#[cfg(test)]
#[path = "postgres_erase_tests.rs"]
mod postgres_erase_tests;

impl ComplianceStore for PostgresStore {
    fn list_subject_items(
        &self,
        bank_id: &str,
        subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query(
                    "SELECT id, category, epistemic_kind FROM items
                     WHERE bank_id = $1 AND subject_id = $2 ORDER BY id",
                    &[&bank_id, &subject_id],
                )
                .am("erase list")?;
            Ok(rows
                .iter()
                .map(|r| ItemIdentity {
                    item_id: r.get(0),
                    category: r.get(1),
                    epistemic_kind: r.get(2),
                })
                .collect())
        })
    }

    // Long transactional orchestration: the SQLite twin carries the same
    // allow (the per-step SQL is sequential and each block is small).
    #[allow(clippy::too_many_lines)]
    fn erase_subject(&self, spec: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        validate_spec(spec)?;
        self.with_client(|client| {
            let mut tx = client.transaction().am("erase begin")?;
            crate::postgres_memory::lock_subject_write_tx(&mut tx, spec.subject_id)?;
            let key_row = tx
                .query_opt(
                    "SELECT shredded_at IS NOT NULL FROM subject_keys
                     WHERE subject_id = $1 FOR UPDATE",
                    &[&spec.subject_id],
                )
                .am("erase key lock")?;
            let key_exists = key_row.is_some();
            // A marker already marked shredded belongs to an earlier attempt of
            // this erasure, so only that retry may read a destroy error as
            // "the key is already gone". A missing row means false: this is a
            // first attempt.
            let already_shredded = key_row.is_some_and(|row| row.get::<_, bool>(0));
            let items = select_items(&mut tx, spec.bank_id, spec.subject_id)?;
            let subject_banks = select_subject_banks(&mut tx, spec.subject_id)?;
            // Persona context is bank-scoped and carries no subject
            // attribution: when the subject has neither items nor a key, its
            // bank's persona context rows are still erasable content (fail
            // closed) so the verified erasure can purge them.
            let persona_context = bank_has_persona_context(&mut tx, spec.bank_id)?;
            if items.is_empty() && subject_banks.is_empty() && !key_exists && !persona_context {
                return Err(AmError::new(
                    ErrorCode::NotFound,
                    format!("no rows for subject `{}` in bank", spec.subject_id),
                ));
            }

            // The DEK is global: fan the derived-structure purge out to every
            // other bank where the subject's items actually reside.
            let mut all_rows = items;
            let mut purge = purge_bank(&mut tx, spec.bank_id, &all_rows)?;
            for bank in &subject_banks {
                if bank == spec.bank_id {
                    continue;
                }
                let rows = select_items(&mut tx, bank, spec.subject_id)?;
                purge.absorb(purge_bank(&mut tx, bank, &rows)?);
                all_rows.extend(rows);
            }

            // Persona context is bank-scoped (persona rows carry no subject
            // attribution): purge it in every bank this verified erasure
            // touches — the requesting bank plus every fan-out bank. Each bank
            // is serialized under its `persona-erase` advisory lock first, so
            // a concurrent persona write transaction for that bank commits
            // (or fails closed on the tombstone) before this erase purges it;
            // each purged bank then gets a bank-level persona erase tombstone
            // so later persona writes into it fail closed instead of
            // resurrecting the purged context (persona rows cannot name the
            // erased subject).
            let mut persona_banks: Vec<String> = vec![spec.bank_id.to_owned()];
            persona_banks.extend(
                subject_banks
                    .iter()
                    .filter(|b| b.as_str() != spec.bank_id)
                    .cloned(),
            );
            persona_banks.sort_unstable();
            persona_banks.dedup();
            let mut persona_rows = 0usize;
            for bank in &persona_banks {
                lock_persona_erase(&mut tx, bank)?;
                let bank_rows = purge_persona_context(&mut tx, bank)?;
                // Tombstone only a bank whose purge destroyed persona content:
                // nothing purged leaves no purged context to resurrect, so the
                // bank stays writable for later persona rows.
                if bank_rows > 0 {
                    write_persona_erase_tombstone(&mut tx, bank, spec.deleted_at)?;
                }
                persona_rows += bank_rows;
            }

            tx.execute(
                "INSERT INTO subject_keys (subject_id, dek_ref, status, shredded_at, created_at, updated_at)
                 VALUES ($1, 'shredded', 'shredded', $2, $2, $2)
                 ON CONFLICT (subject_id) DO UPDATE SET
                   dek_ref = 'shredded', status = 'shredded',
                   shredded_at = EXCLUDED.shredded_at, updated_at = EXCLUDED.updated_at",
                &[&spec.subject_id, &spec.deleted_at],
            )
            .am("erase subject_keys")?;
            // The purge and the shred marker commit before the irreversible
            // step that follows them, so a crash inside the KMS destroy leaves
            // the subject closed for writes instead of leaving a destroyed key
            // and no marker. The committed marker is also what suppresses every
            // later subject-keyed write, so the second transaction below takes
            // no advisory lock of its own.
            tx.commit().am("erase purge commit")?;

            // A retry that reaches this point after a crash can meet a KMS that
            // refuses to destroy an already destroyed key, so a destroy error is
            // tolerated only when an earlier attempt of this erasure already
            // marked the subject shredded and the probe proves the key is gone.
            let destroyed = self.kms.destroy_dek(spec.subject_id);
            let probe = self.kms.ensure_dek(spec.subject_id);
            let key_gone = matches!(&probe, Err(e) if e.code == ErrorCode::ErasedSubject);
            if let Err(err) = destroyed {
                if !(already_shredded && key_gone) {
                    return Err(err);
                }
            } else if !key_gone {
                return Err(AmError::new(
                    ErrorCode::Internal,
                    "DEK destruction probe did not observe a destroyed key",
                ));
            }

            // The tombstone rows and the audit row commit only after the
            // destroy, so a retry re-inserts them only when the previous attempt
            // never committed them.
            let mut tx = client.transaction().am("erase ledger begin")?;

            let subject_hash = sha256_hex(spec.salt, spec.subject_id);
            let mut item_ids = Vec::with_capacity(all_rows.len());
            let mut item_hashes = Vec::with_capacity(all_rows.len());
            let mut categories: Vec<String> = Vec::new();
            let mut tombstones = 0usize;
            for row in &all_rows {
                let item_hash = sha256_hex(spec.salt, &row.item_id);
                insert_tombstone(
                    &mut tx,
                    spec,
                    &subject_hash,
                    Some(&item_hash),
                    row.category.as_deref(),
                )?;
                if let Some(cat) = &row.category
                    && !categories.contains(cat) {
                        categories.push(cat.clone());
                    }
                item_ids.push(row.item_id.clone());
                item_hashes.push(item_hash);
                tombstones += 1;
            }
            if all_rows.is_empty() {
                insert_tombstone(&mut tx, spec, &subject_hash, None, None)?;
                tombstones += 1;
            }
            let detail = serde_json::json!({
                "request_id": spec.request_id,
                "subject_hash": subject_hash,
                "reason_code": spec.reason_code,
                "items": all_rows.len(),
                "triples_purged": purge.triples,
                "persona_context_rows": persona_rows
            })
            .to_string();
            tx.execute(
                "INSERT INTO audit_events (
                    id, bank_id, operation, item_id, actor, detail_json, created_at
                 ) VALUES ($1, $2, 'compliance_erase', NULL, 'system', $3, $4)",
                &[
                    &format!("ct-audit-{}-{}", spec.request_id, subject_hash),
                    &spec.bank_id,
                    &detail,
                    &spec.deleted_at,
                ],
            )
            .am("erase audit")?;
            tx.commit().am("erase commit")?;

            Ok(SubjectEraseOutcome {
                item_ids,
                categories,
                item_hashes,
                subject_hash,
                vectors_purged: purge.vectors,
                assoc_edges_purged: purge.edges,
                memtree_nodes_purged: purge.memtree,
                triples_purged: purge.triples,
                derived_items_purged: purge.derived_items.len(),
                persona_context_rows_purged: persona_rows,
                tombstones_written: tombstones,
            })
        })
    }

    fn subject_dek_destroyed(&self, subject_id: &str) -> Result<bool, AmError> {
        match self.kms.ensure_dek(subject_id) {
            Ok(_) => Ok(false),
            Err(e) if e.code == ErrorCode::ErasedSubject => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn read_tombstones(
        &self,
        bank_id: &str,
        request_id: &str,
    ) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        crate::postgres_erase_read::tombstones_where(
            self,
            "bank_id = $1 AND request_id = $2",
            &[&bank_id, &request_id],
        )
    }

    fn list_tombstones(&self, bank_id: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        crate::postgres_erase_read::tombstones_where(self, "bank_id = $1", &[&bank_id])
    }
}

impl PostgresStore {
    /// The bank set the erasure effect fans out to for one subject: the
    /// requesting bank plus every bank holding the subject's items, sorted.
    /// This is the same `SELECT DISTINCT` the erasure transaction reads, so
    /// the coordination seam takes its live set from here and never from a
    /// separately configured list.
    ///
    /// # Errors
    /// Storage failures.
    pub fn erase_bank_set(&self, bank_id: &str, subject_id: &str) -> Result<Vec<String>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query(
                    "SELECT DISTINCT bank_id FROM items WHERE subject_id = $1 ORDER BY bank_id",
                    &[&subject_id],
                )
                .am("erase bank set")?;
            let mut banks: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
            if !banks.iter().any(|bank| bank == bank_id) {
                banks.push(bank_id.to_owned());
                banks.sort();
            }
            Ok(banks)
        })
    }
}

impl BankScopeStore for PostgresStore {
    fn erase_bank_set(&self, bank_id: &str, subject_id: &str) -> Result<Vec<String>, AmError> {
        PostgresStore::erase_bank_set(self, bank_id, subject_id)
    }
}
