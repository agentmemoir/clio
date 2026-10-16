// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the compliance-erasure persistence contract:
//! crypto-shredding a data subject's DEK, purging derived structures, and
//! writing content-free tombstones.
//!
//! ## Owns
//! - `ComplianceStore` trait and its request/outcome types
//! - The atomic purge + tombstone transaction contract (backend SQL per driver)
//! - Shared erasure input validation and the per-item purge record type
//!
//! ## Does not own
//! - Erasure authorization, confirmation gating, or telemetry (see `clio-compliance`)
//! - KMS key material internals (see [`crate::dek`])
//! - Derived-structure recomputation scheduling (`clio-write` memory)
//!
//! ## Boundary
//!
//! Changes to which rows the erasure transaction purges should normally require
//! changes here and in the per-backend `*_erase` modules. Changes to the
//! compliance workflow should not require changes here.
//!
//! Keep this module focused on the erasure persistence contract.

use crate::model::ItemIdentity;
use clio_types::{AmError, ErrorCode};

/// One erased item row (metadata only), shared by the backend drivers.
pub(crate) struct EraseRow {
    /// Item row id to purge derived structures for.
    pub(crate) item_id: String,
    /// Semantic category, copied into the tombstone.
    pub(crate) category: Option<String>,
}

/// The SPO triple rows one purge step deleted, shared by the backend drivers.
///
/// The ids travel with the count because the erasure still has to blank the
/// sync-journal and dead-letter copies of the same triple payloads, and those
/// rows are keyed by triple id rather than by item id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PurgedTriples {
    /// Triple rows deleted.
    pub(crate) rows: usize,
    /// Ids of the deleted triple rows.
    pub(crate) ids: Vec<String>,
}

/// What one bank's derived-row purge removed, shared by the backend drivers.
///
/// A named tally rather than a tuple, because the erasure transaction has to
/// fold several banks together and has to carry the deleted triple ids forward
/// to blank the sync-journal and dead-letter copies of the same content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BankPurge {
    /// Dense vector rows deleted.
    pub(crate) vectors: usize,
    /// Association edges deleted.
    pub(crate) edges: usize,
    /// MemTree nodes deleted or re-marked dirty.
    pub(crate) memtree: usize,
    /// SPO triple rows deleted.
    pub(crate) triples: usize,
    /// Ids of the deleted triple rows, so their sync-derived payload copies can
    /// be blanked in the same transaction.
    pub(crate) triple_ids: Vec<String>,
    /// Derived (hub-distilled) items purged because they referenced an erased
    /// item.
    pub(crate) derived_items: Vec<String>,
}

impl BankPurge {
    /// Fold another bank's tally into this one.
    pub(crate) fn absorb(&mut self, other: BankPurge) {
        self.vectors += other.vectors;
        self.edges += other.edges;
        self.memtree += other.memtree;
        self.triples += other.triples;
        self.triple_ids.extend(other.triple_ids);
        self.derived_items.extend(other.derived_items);
    }
}

/// Validate the shared erasure-input contract before opening a transaction.
pub(crate) fn validate_spec(spec: &SubjectEraseSpec<'_>) -> Result<(), AmError> {
    if spec.bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if spec.subject_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "subject_id is required",
        ));
    }
    if spec.request_id.trim().is_empty() || spec.legal_basis.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "request_id and legal_basis are required",
        ));
    }
    Ok(())
}

/// Inputs for one crypto-shred transaction.
#[derive(Debug, Clone)]
pub struct SubjectEraseSpec<'a> {
    /// Bank isolation key.
    pub bank_id: &'a str,
    /// Data-subject id whose DEK is destroyed.
    pub subject_id: &'a str,
    /// Caller compliance tracking id.
    pub request_id: &'a str,
    /// Legal basis (e.g. `GDPR_ART_17`).
    pub legal_basis: &'a str,
    /// Machine reason code (e.g. `COMPLIANCE_CRYPTO_SHRED`).
    pub reason_code: &'a str,
    /// Erasure timestamp (UTC text) recorded with the tombstone.
    pub deleted_at: &'a str,
    /// Deployment salt for the one-way tombstone hashes.
    pub salt: &'a [u8],
}

/// Content-free tombstone row as read back for verification.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComplianceTombstoneView {
    /// Request id that authorized the erasure.
    pub request_id: String,
    /// Legal basis recorded for the erasure.
    pub legal_basis: String,
    /// One-way hash of the data-subject id.
    pub subject_hash: String,
    /// One-way hash of the erased item id (`None` for a subject-level row).
    pub item_id_hash: Option<String>,
    /// Semantic category, when the erased item was semantic.
    pub category: Option<String>,
    /// Deletion reason code.
    pub reason_code: String,
    /// Erasure timestamp.
    pub deleted_at: String,
}

/// Result of one crypto-shred transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectEraseOutcome {
    /// Plaintext item ids that were erased (never persisted in tombstones).
    pub item_ids: Vec<String>,
    /// Semantic categories seen among the erased items.
    pub categories: Vec<String>,
    /// One-way hashes, aligned with `item_ids`.
    pub item_hashes: Vec<String>,
    /// One-way hash of the data-subject id.
    pub subject_hash: String,
    /// Dense vector rows purged.
    pub vectors_purged: usize,
    /// Association edges purged.
    pub assoc_edges_purged: usize,
    /// Derived MemTree nodes purged or re-marked dirty.
    pub memtree_nodes_purged: usize,
    /// SPO triple rows deleted, together with the sync-journal and dead-letter
    /// payload copies of the same triples. Counted because a triple row keeps
    /// its plaintext `subject` + `predicate` identity key after the DEK is
    /// destroyed, so the erasure is only complete once the row is gone.
    pub triples_purged: usize,
    /// Derived (hub-distilled) items purged because they referenced an erased item.
    pub derived_items_purged: usize,
    /// Persona rows whose sealed `context_cipher` was cleared (persona
    /// context is bank-scoped, so any verified erasure in its bank purges it).
    pub persona_context_rows_purged: usize,
    /// Tombstone rows written.
    pub tombstones_written: usize,
}

/// Persistence contract for the compliance erasure path.
///
/// Implementations MUST perform the DEK destruction, derived-row purge, and
/// tombstone insert for one subject with no plaintext kept in the ledger.
pub trait ComplianceStore: Send + Sync {
    /// Bank-scoped item identities belonging to a data subject (no content/DEK).
    ///
    /// # Errors
    /// Storage failures.
    fn list_subject_items(
        &self,
        bank_id: &str,
        subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, AmError>;

    /// Destroy the subject DEK and purge derived rows, writing tombstones.
    ///
    /// Returns [`clio_types::ErrorCode::NotFound`] when the subject has
    /// neither items, nor a key record, nor persona context rows in the bank
    /// (no invalid tombstone is written). Persona context is bank-scoped, so
    /// a bank holding persona context rows is treated as holding erasable
    /// content and the erase proceeds with a context purge.
    ///
    /// # Errors
    /// Missing subject, validation, crypto, or storage failures.
    fn erase_subject(&self, spec: &SubjectEraseSpec<'_>) -> Result<SubjectEraseOutcome, AmError>;

    /// True when the subject's DEK has already been destroyed (negative probe).
    ///
    /// # Errors
    /// KMS provider failures other than a destroyed key.
    fn subject_dek_destroyed(&self, subject_id: &str) -> Result<bool, AmError>;

    /// Content-free tombstone rows written for one request (verification aid).
    ///
    /// # Errors
    /// Storage failures.
    fn read_tombstones(
        &self,
        bank_id: &str,
        request_id: &str,
    ) -> Result<Vec<ComplianceTombstoneView>, AmError>;

    /// Every content-free tombstone row in the bank (export provenance).
    ///
    /// # Errors
    /// Storage failures.
    fn list_tombstones(&self, bank_id: &str) -> Result<Vec<ComplianceTombstoneView>, AmError>;
}

/// Read the live bank set an erasure fans out to for one data subject.
///
/// Kept apart from [`ComplianceStore`] so the coordination seam can take the
/// live set through a `dyn` object without forcing every existing
/// `dyn ComplianceStore` test double to implement this method.
pub trait BankScopeStore: Send + Sync {
    /// The requesting bank plus every bank holding the subject's items,
    /// sorted; read from the live items, never a configured list.
    ///
    /// # Errors
    /// Storage failures.
    fn erase_bank_set(&self, bank_id: &str, subject_id: &str) -> Result<Vec<String>, AmError>;
}

#[cfg(test)]
#[path = "compliance_store_bank_scope_tests.rs"]
mod compliance_store_bank_scope_tests;

#[cfg(test)]
#[path = "compliance_store_context_store_tests.rs"]
mod compliance_store_context_store_tests;

#[cfg(test)]
#[path = "compliance_store_erase_distilled_unit_tests.rs"]
mod compliance_store_erase_distilled_unit_tests;

#[cfg(test)]
#[path = "compliance_store_erase_preserve_tests.rs"]
mod compliance_store_erase_preserve_tests;

#[cfg(test)]
#[path = "postgres_erase_retry_tests.rs"]
mod postgres_erase_retry_tests;

#[cfg(test)]
#[path = "compliance_store_erase_tests.rs"]
pub(crate) mod compliance_store_erase_tests;

#[cfg(test)]
#[path = "compliance_store_pg_export_read_tests.rs"]
mod compliance_store_pg_export_read_tests;

#[cfg(test)]
#[path = "compliance_store_subject_shred_guard_tests.rs"]
mod compliance_store_subject_shred_guard_tests;
