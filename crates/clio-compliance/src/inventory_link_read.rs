// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the store reads the subject-family mapping
//! binds to: one subject-scoped read per family the store can attribute to a
//! data subject, expressed as a single trait the mapping holds as a trait
//! object, plus the blanket implementation both store backends satisfy.
//!
//! ## Owns
//! - `FamilyLinkRead`, the subject-scoped reads one mapping run needs
//! - The blanket implementation over `Store + ComplianceStore + HistoryStore +
//!   BeliefStore`, which forwards to the subject-scoped history reads
//!
//! ## Does not own
//! - The subject restriction itself: each store read enforces it
//! - The family mapping and its named omissions (`inventory_map`)
//! - The adapter from an export's split repository views
//!   (`crate::export_subject_selection`)
//!
//! ## Boundary
//!
//! Changes to which subject-scoped reads a mapping run needs should normally
//! require changes here. Changes to how a family binds to a subject, or to the
//! store's own SQL, should not.
//!
//! Keep this module focused on the subject-scoped store reads.

use clio_store::{BeliefStore, ComplianceStore, HistoryStore, ItemIdentity, Store};
use clio_types::{AmError, BeliefObject, FailureRecord, TaskRecord, TripleRecord};

/// The subject-scoped store reads one subject-family mapping needs.
///
/// Each read that has a subject column is scoped to the subject, so the mapping never filters another subject's rows out of a bank-wide page. Both store backends satisfy the blanket implementation, so the mapping binds to either one without a change in `clio-store`. A caller that holds the four repository views separately implements this over an adapter.
pub(crate) trait FamilyLinkRead {
    /// The subject's own item identities, selected by its stored `subject_id`.
    fn list_subject_items(
        &self,
        bank_id: &str,
        subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, AmError>;
    /// Every triple in the bank; the mapping keeps those carried by the
    /// subject's own items.
    fn list_triples(&self, bank_id: &str) -> Result<Vec<TripleRecord>, AmError>;
    /// The subject's own task history, capped at `limit`.
    ///
    /// The cap applies to the subject's own rows: rows of other subjects in the same bank consume no part of it.
    fn list_tasks(
        &self,
        bank_id: &str,
        subject_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskRecord>, AmError>;
    /// The subject's own failure history, capped at `limit`.
    ///
    /// The cap applies to the subject's own rows: rows of other subjects in the same bank consume no part of it.
    fn list_failures(
        &self,
        bank_id: &str,
        subject_id: &str,
        limit: usize,
    ) -> Result<Vec<FailureRecord>, AmError>;
    /// Every belief object in the bank; the mapping keeps the subject's own.
    fn list_beliefs(&self, bank_id: &str) -> Result<Vec<BeliefObject>, AmError>;
}

impl<T: Store + ComplianceStore + HistoryStore + BeliefStore> FamilyLinkRead for T {
    fn list_subject_items(
        &self,
        bank_id: &str,
        subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, AmError> {
        ComplianceStore::list_subject_items(self, bank_id, subject_id)
    }
    fn list_triples(&self, bank_id: &str) -> Result<Vec<TripleRecord>, AmError> {
        Store::list_triples(self, bank_id)
    }
    fn list_tasks(
        &self,
        bank_id: &str,
        subject_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskRecord>, AmError> {
        HistoryStore::list_tasks_for_subject(self, bank_id, subject_id, limit)
    }
    fn list_failures(
        &self,
        bank_id: &str,
        subject_id: &str,
        limit: usize,
    ) -> Result<Vec<FailureRecord>, AmError> {
        HistoryStore::list_failures_for_subject(self, bank_id, subject_id, limit)
    }
    fn list_beliefs(&self, bank_id: &str) -> Result<Vec<BeliefObject>, AmError> {
        BeliefStore::list_beliefs(self, bank_id)
    }
}

#[cfg(test)]
#[path = "inventory_link_read_tests.rs"]
mod inventory_link_read_tests;
