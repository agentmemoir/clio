// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite compliance-erasure transactions:
//! the derived-row purge and shred marker that commit first, the DEK
//! destruction that follows them, and the content-free tombstone insert.
//!
//! ## Owns
//! - `ComplianceStore` implementation for [`SqliteStore`]
//! - The SQLite erasure split: the purge and shred marker commit, then the DEK
//!   destruction, then the tombstones and audit row
//! - The bank-level persona erase tombstone written for every bank whose
//!   persona context this erasure purges (fail-closes later persona writes)
//! - Cross-bank fan-out of the derived-structure purge, which commits before
//!   the DEK is destroyed
//!
//! ## Does not own
//! - Erasure authorization/confirmation or telemetry (`clio-compliance`)
//! - Postgres parity (see `postgres_erase`)
//!
//! ## Boundary
//!
//! Changes to transaction ordering, tombstones, or the KMS probe should normally
//! require changes here. Changes to the purge SQL should require
//! `sqlite_erase_purge`; changes to the contract should require `compliance_store`.
//!
//! Keep this module focused on the SQLite erasure transaction.

use crate::compliance_store::{
    BankScopeStore, ComplianceStore, ComplianceTombstoneView, SubjectEraseOutcome,
    SubjectEraseSpec, validate_spec,
};
use crate::erase_hash::sha256_hex;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use rusqlite::params;

impl SqliteStore {
    fn tombstones_where(
        &self,
        where_sql: &str,
        values: impl rusqlite::Params,
    ) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        self.with_conn(|conn| {
            let sql = format!(
                "SELECT request_id, legal_basis, subject_hash, item_id_hash, category,
                        reason_code, deleted_at
                 FROM compliance_tombstones
                 WHERE {where_sql}
                 ORDER BY item_id_hash NULLS LAST, request_id, deleted_at"
            );
            let mut stmt = conn.prepare(&sql).am("tombstones prepare")?;
            let rows = stmt
                .query_map(values, |r| {
                    Ok(ComplianceTombstoneView {
                        request_id: r.get(0)?,
                        legal_basis: r.get(1)?,
                        subject_hash: r.get(2)?,
                        item_id_hash: r.get(3)?,
                        category: r.get(4)?,
                        reason_code: r.get(5)?,
                        deleted_at: r.get(6)?,
                    })
                })
                .am("tombstones query")?;
            rows.collect::<Result<Vec<_>, _>>().am("tombstone rows")
        })
    }
}
use crate::model::ItemIdentity;
use crate::sqlite_erase_purge::{
    bank_has_persona_context, purge_bank, purge_persona_context, select_items,
    select_subject_banks, write_persona_erase_tombstone,
};
use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, OptionalExtension};

impl ComplianceStore for SqliteStore {
    fn list_subject_items(
        &self,
        bank_id: &str,
        subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, category, epistemic_kind FROM items
                     WHERE bank_id = ?1 AND subject_id = ?2 ORDER BY id",
                )
                .am("erase list prepare")?;
            let rows = stmt
                .query_map(params![bank_id, subject_id], |r| {
                    Ok(ItemIdentity {
                        item_id: r.get(0)?,
                        category: r.get(1)?,
                        epistemic_kind: r.get(2)?,
                    })
                })
                .am("erase list query")?;
            rows.collect::<Result<Vec<_>, _>>().am("erase list rows")
        })
    }

    #[allow(clippy::too_many_lines)]
    fn erase_subject(&self, spec: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError> {
        validate_spec(spec)?;
        self.with_conn(|conn| {
            // Destroying the DEK makes every sealed body unreadable, but a
            // DELETE only unlinks a row: SQLite leaves the freed pages' bytes in
            // the file, where a copy of the database, a backup, or a volume
            // snapshot would still read them. `secure_delete` overwrites what it
            // frees, so the attestation is true at the byte level and not just
            // at the row level. It is scoped to this call, which spans both
            // erasure transactions (the store's connection lock is held for the
            // whole call), and restored on the way out, so ordinary writes pay
            // nothing for it.
            let prior_secure_delete = read_secure_delete(conn)?;
            conn.execute_batch("PRAGMA secure_delete = ON")
                .am("erase enable secure_delete")?;
            let erased = erase_subject_tx(self, conn, spec);
            let restored = write_secure_delete(conn, prior_secure_delete);
            match (erased, restored) {
                (Ok(outcome), Ok(())) => Ok(outcome),
                (Err(e), _) => Err(e),
                (Ok(_), Err(e)) => Err(e),
            }
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
        self.tombstones_where(
            "bank_id = ?1 AND request_id = ?2",
            params![bank_id, request_id],
        )
    }

    fn list_tombstones(&self, bank_id: &str) -> Result<Vec<ComplianceTombstoneView>, AmError> {
        self.tombstones_where("bank_id = ?1", params![bank_id])
    }
}

impl SqliteStore {
    /// The bank set the erasure effect fans out to for one subject: the
    /// requesting bank plus every bank holding the subject's items, sorted.
    /// This is the same `SELECT DISTINCT` the erasure transaction reads, so
    /// the coordination seam takes its live set from here and never from a
    /// separately configured list.
    ///
    /// # Errors
    /// Storage failures.
    pub fn erase_bank_set(&self, bank_id: &str, subject_id: &str) -> Result<Vec<String>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT bank_id FROM items
                     WHERE subject_id = ?1 ORDER BY bank_id",
                )
                .am("erase bank set prepare")?;
            let rows = stmt
                .query_map(params![subject_id], |r| r.get(0))
                .am("erase bank set query")?;
            let mut banks = rows
                .collect::<Result<Vec<String>, _>>()
                .am("erase bank set rows")?;
            if !banks.iter().any(|bank| bank == bank_id) {
                banks.push(bank_id.to_owned());
                banks.sort();
            }
            Ok(banks)
        })
    }
}

impl BankScopeStore for SqliteStore {
    fn erase_bank_set(&self, bank_id: &str, subject_id: &str) -> Result<Vec<String>, AmError> {
        SqliteStore::erase_bank_set(self, bank_id, subject_id)
    }
}

/// Read the connection's current `secure_delete` setting.
fn read_secure_delete(conn: &Connection) -> Result<bool, AmError> {
    let raw: i64 = conn
        .query_row("PRAGMA secure_delete", [], |r| r.get(0))
        .am("erase read secure_delete")?;
    Ok(raw != 0)
}

/// Restore a previously read `secure_delete` setting.
fn write_secure_delete(conn: &Connection, on: bool) -> Result<(), AmError> {
    conn.execute_batch(if on {
        "PRAGMA secure_delete = ON"
    } else {
        "PRAGMA secure_delete = OFF"
    })
    .am("erase restore secure_delete")
}

/// The erasure transactions' body, run with the connection's `secure_delete`
/// enabled by the caller.
#[allow(clippy::too_many_lines)]
fn erase_subject_tx(
    store: &SqliteStore,
    conn: &mut Connection,
    spec: &SubjectEraseSpec<'_>,
) -> Result<SubjectEraseOutcome, AmError> {
    let tx = conn.transaction().am("erase begin")?;
    let items = select_items(&tx, spec.bank_id, spec.subject_id)?;
    let subject_banks = select_subject_banks(&tx, spec.subject_id)?;
    let key_row = tx
        .query_row(
            "SELECT shredded_at IS NOT NULL FROM subject_keys WHERE subject_id = ?1",
            params![spec.subject_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .am("erase key check")?;
    let key_exists = key_row.is_some();
    // Subject-scoped: true when the subject was already marked shredded
    // before this attempt, by an earlier attempt or another bank's
    // erasure, so only that state may tolerate a destroy error.
    let already_shredded = key_row.is_some_and(|shredded| shredded != 0);
    // Persona context is bank-scoped and carries no subject
    // attribution: when the subject has neither items nor a key, its
    // bank's persona context rows are still erasable content (fail
    // closed) so the verified erasure can purge them.
    let persona_context = bank_has_persona_context(&tx, spec.bank_id)?;
    if items.is_empty() && subject_banks.is_empty() && !key_exists && !persona_context {
        return Err(AmError::new(
            ErrorCode::NotFound,
            format!("no rows for subject `{}` in bank", spec.subject_id),
        ));
    }

    // The DEK is global: fan the derived-structure purge out to every
    // other bank where the subject's items actually reside.
    let mut all_rows = items;
    let mut purge = purge_bank(&tx, spec.bank_id, &all_rows)?;
    for bank in &subject_banks {
        if bank == spec.bank_id {
            continue;
        }
        let rows = select_items(&tx, bank, spec.subject_id)?;
        purge.absorb(purge_bank(&tx, bank, &rows)?);
        all_rows.extend(rows);
    }

    // Persona context is bank-scoped (persona rows carry no subject
    // attribution): purge it in every bank this verified erasure
    // touches — the requesting bank plus every fan-out bank. A bank
    // whose purge destroyed persona content gets a bank-level persona
    // erase tombstone so later persona writes into it fail closed
    // instead of resurrecting the purged context (persona rows cannot
    // name the erased subject); a bank with nothing purged has no
    // purged context to resurrect, so it stays writable.
    let mut persona_rows = purge_persona_context(&tx, spec.bank_id)?;
    if persona_rows > 0 {
        write_persona_erase_tombstone(&tx, spec.bank_id, spec.deleted_at)?;
    }
    for bank in &subject_banks {
        if bank.as_str() == spec.bank_id {
            continue;
        }
        let bank_rows = purge_persona_context(&tx, bank)?;
        if bank_rows > 0 {
            write_persona_erase_tombstone(&tx, bank, spec.deleted_at)?;
        }
        persona_rows += bank_rows;
    }

    tx.execute(
        "INSERT INTO subject_keys (subject_id, dek_ref, status, shredded_at, created_at, updated_at)
         VALUES (?1, 'shredded', 'shredded', ?2, ?2, ?2)
         ON CONFLICT(subject_id) DO UPDATE SET
           dek_ref = 'shredded', status = 'shredded',
           shredded_at = excluded.shredded_at, updated_at = excluded.updated_at",
        params![spec.subject_id, spec.deleted_at],
    )
    .am("erase subject_keys")?;
    // The purge and the shred marker commit before the irreversible step that
    // follows them, so a crash inside the KMS destroy leaves the subject closed
    // for writes instead of leaving a destroyed key and no marker.
    tx.commit().am("erase purge commit")?;

    // A KMS may refuse an already destroyed key, so a destroy error is
    // tolerated only when the subject was marked shredded by an earlier
    // attempt or by another bank's erasure, and the probe finds no key.
    let destroyed = store.kms.destroy_dek(spec.subject_id);
    let probe = store.kms.ensure_dek(spec.subject_id);
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

    // The tombstone rows and the audit row commit only after the destroy, so a
    // retry re-inserts them only when the previous attempt never committed them.
    let tx = conn.transaction().am("erase ledger begin")?;

    let subject_hash = sha256_hex(spec.salt, spec.subject_id);
    let mut item_ids = Vec::with_capacity(all_rows.len());
    let mut item_hashes = Vec::with_capacity(all_rows.len());
    let mut categories: Vec<String> = Vec::new();
    let mut tombstones = 0usize;
    for row in &all_rows {
        let item_hash = sha256_hex(spec.salt, &row.item_id);
        insert_tombstone(
            &tx,
            spec,
            &subject_hash,
            Some(&item_hash),
            row.category.as_deref(),
        )?;
        if let Some(cat) = &row.category
            && !categories.contains(cat)
        {
            categories.push(cat.clone());
        }
        item_ids.push(row.item_id.clone());
        item_hashes.push(item_hash);
        tombstones += 1;
    }
    if all_rows.is_empty() {
        insert_tombstone(&tx, spec, &subject_hash, None, None)?;
        tombstones += 1;
    }
    let detail = serde_json::json!({
        "request_id": spec.request_id,
        "subject_hash": subject_hash,
        "reason_code": spec.reason_code,
        "items": all_rows.len(),
        "triples_purged": purge.triples,
        "persona_context_rows": persona_rows,
        "persona_context_rows_scope": "this erasure attempt"
    })
    .to_string();
    tx.execute(
        "INSERT INTO audit_events (
            id, bank_id, operation, item_id, actor, detail_json, created_at
         ) VALUES (?1, ?2, 'compliance_erase', NULL, 'system', ?3, ?4)",
        params![
            format!("ct-audit-{}-{}", spec.request_id, subject_hash),
            spec.bank_id,
            detail,
            spec.deleted_at,
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
}

fn insert_tombstone(
    tx: &rusqlite::Transaction<'_>,
    spec: &SubjectEraseSpec<'_>,
    subject_hash: &str,
    item_hash: Option<&str>,
    category: Option<&str>,
) -> Result<(), AmError> {
    let suffix = item_hash.unwrap_or(subject_hash);
    tx.execute(
        "INSERT INTO compliance_tombstones (
            id, bank_id, request_id, legal_basis, subject_hash, item_id_hash,
            category, reason_code, deleted_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            format!("ct-{}-{}", spec.request_id, suffix),
            spec.bank_id,
            spec.request_id,
            spec.legal_basis,
            subject_hash,
            item_hash,
            category,
            spec.reason_code,
            spec.deleted_at,
        ],
    )
    .am("tombstone insert")?;
    Ok(())
}
