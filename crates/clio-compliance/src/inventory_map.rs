// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-family mapping of the
//! compliance scope ledger: binding every planned family to the named
//! subject's own stored subject link, or to a named omission when this store
//! holds no per-subject read for that family.
//!
//! ## Owns
//! - `subject_family_mapping` and one `FamilySelection` per planned ledger row
//! - The per-family class, the selected ids and counts, the omitted counts,
//!   and every named omission reason
//!
//! ## Does not own
//! - The fixed row table (`inventory_plan`) and the row schema (`inventory_rows`)
//! - The scope ledger itself (`inventory`) and the store reads (`clio-store`)
//! - Deciding which records belong to a subject: each store read enforces the
//!   subject restriction itself
//!
//! ## Boundary
//!
//! Changes to how a family binds to a subject should normally require changes
//! here. Changes to the fixed row table or to the store's own reads should
//! not.
//!
//! Keep this module focused on the subject-family mapping.

use super::{ROW_PLAN, RowPlan, missing_tables, pattern_matches};
use crate::export::{HISTORY_LIMIT, hit_history_limit};
use crate::inventory::{InventoryBackend, InventoryFacts, Readability, RowSource};
use clio_store::{BeliefStore, ComplianceStore, HistoryStore, ItemIdentity, Store};
use clio_types::{AmError, BeliefObject, FailureRecord, TaskRecord, TripleRecord};

/// The store reads one subject-family mapping needs.
///
/// Both store backends satisfy the blanket implementation, so the mapping
/// binds to either one without a change in `clio-store`. A caller that holds
/// the four repository views separately implements this over an adapter.
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
    /// The bank's task history, capped at `limit`.
    fn list_tasks(&self, bank_id: &str, limit: usize) -> Result<Vec<TaskRecord>, AmError>;
    /// The bank's failure history, capped at `limit`.
    fn list_failures(&self, bank_id: &str, limit: usize) -> Result<Vec<FailureRecord>, AmError>;
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
    fn list_tasks(&self, bank_id: &str, limit: usize) -> Result<Vec<TaskRecord>, AmError> {
        HistoryStore::list_tasks(self, bank_id, limit)
    }
    fn list_failures(&self, bank_id: &str, limit: usize) -> Result<Vec<FailureRecord>, AmError> {
        HistoryStore::list_failures(self, bank_id, limit)
    }
    fn list_beliefs(&self, bank_id: &str) -> Result<Vec<BeliefObject>, AmError> {
        BeliefStore::list_beliefs(self, bank_id)
    }
}

/// How one planned family binds to the mapped subject on this store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FamilyClass {
    /// The store has a per-subject read for the family, and the subject's own
    /// stored link selects its records.
    Selected,
    /// The row names a stored link, but the store has no per-subject read for
    /// it, so the mapping cannot select any of its measured rows.
    OtherOnly,
    /// No subject column: the records are shared or bank-scoped and can never
    /// be attributed to one subject.
    InseparableShared,
    /// A one-way hashed link only: the subject id cannot be recovered.
    UnknownLink,
    /// No stored table: the row describes a path, a location, or a process
    /// that has no durable record.
    NotStored,
}

/// One planned family bound to the mapped subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FamilySelection {
    /// The stable family name, one per planned ledger row.
    pub(crate) family: &'static str,
    /// How this family binds to the subject.
    pub(crate) class: FamilyClass,
    /// Records the family's own stored subject link selected for the subject.
    pub(crate) selected: u64,
    /// The ids those reads returned; never record contents.
    pub(crate) selected_ids: Vec<String>,
    /// Store-wide measured rows the mapping did not select for this subject.
    pub(crate) omitted: u64,
    /// The named reason this family is incomplete, when one applies.
    pub(crate) omission: Option<String>,
    /// True iff this family carries no named omission: every store-wide
    /// measured row is selected and no cap or missing table applies. This is
    /// an internal store-wide accounting flag, not an export verdict: the
    /// fixed plan always includes families with no per-subject read, so a
    /// full mapping stays incomplete and a subject filter alone never decides
    /// an export.
    pub(crate) complete: bool,
}

/// One mapping run's inputs.
pub(crate) struct SubjectFamilyInput<'a> {
    /// The store, read through its own public reads only.
    pub(crate) store: &'a dyn FamilyLinkRead,
    /// The bank the subject's records are read from.
    pub(crate) bank_id: &'a str,
    /// The data subject every planned family is mapped for.
    pub(crate) subject_id: &'a str,
    /// The live-store facts the measured row counts come from, store-wide.
    pub(crate) facts: &'a InventoryFacts,
    /// The scope ledger's backend, supplied by the caller.
    pub(crate) backend: InventoryBackend,
}

/// Every planned family bound to one subject on one store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubjectFamilyMapping {
    /// Which store backend the mapping describes.
    pub(crate) backend: InventoryBackend,
    /// The data subject every family was mapped for.
    pub(crate) subject_id: String,
    /// One selection per planned ledger row, in report order.
    pub(crate) families: Vec<FamilySelection>,
    /// The records all families selected for the subject.
    pub(crate) selected_total: u64,
    /// The store-wide measured rows no family selected for the subject.
    pub(crate) omitted_total: u64,
    /// True iff every family is complete. Internal only: with the fixed plan
    /// this stays false, because shared, hashed-link, and not-stored families
    /// always name an omission. Export completeness is decided separately by
    /// the export path from its own link, cap, and filter conditions.
    pub(crate) complete: bool,
    /// Every named omission, prefixed with its family.
    pub(crate) omissions: Vec<String>,
}

/// Bind every planned family to `subject_id`'s stored links on one store.
///
/// One selection is produced per planned ledger row, in report order. A
/// family with no per-subject read, no subject column, a hashed link, or no
/// stored table selects nothing and names its omission; the call never errors
/// or stays silent because a link is missing, and it never names another
/// subject.
///
/// # Errors
/// Storage read failures from the store's own public reads.
pub(crate) fn subject_family_mapping(
    input: &SubjectFamilyInput<'_>,
) -> Result<SubjectFamilyMapping, AmError> {
    let records = SubjectRecords::read(input.store, input.bank_id, input.subject_id)?;
    let tables = facts_tables(input.facts);
    let mut families = Vec::with_capacity(ROW_PLAN.len());
    let mut omissions = Vec::new();
    for plan in ROW_PLAN {
        let selection = family_selection(plan, &records, tables);
        if let Some(reason) = &selection.omission {
            omissions.push(format!("{}: {reason}", selection.family));
        }
        families.push(selection);
    }
    let selected_total = families.iter().map(|selection| selection.selected).sum();
    let omitted_total = families.iter().map(|selection| selection.omitted).sum();
    let complete = families.iter().all(|selection| selection.complete);
    Ok(SubjectFamilyMapping {
        backend: input.backend,
        subject_id: input.subject_id.to_owned(),
        families,
        selected_total,
        omitted_total,
        complete,
        omissions,
    })
}

/// The records the subject's own stored links select, read once per mapping
/// through the store's existing public reads.
struct SubjectRecords {
    items: Vec<ItemIdentity>,
    triples: Vec<TripleRecord>,
    tasks: Vec<TaskRecord>,
    failures: Vec<FailureRecord>,
    beliefs: Vec<BeliefObject>,
    tasks_capped: bool,
    failures_capped: bool,
}

impl SubjectRecords {
    /// Read every per-subject record this mapping can select.
    fn read(store: &dyn FamilyLinkRead, bank_id: &str, subject_id: &str) -> Result<Self, AmError> {
        let items = store.list_subject_items(bank_id, subject_id)?;
        // A triple's subject link runs through its carrier item, never
        // through its edge id: read the bank's triples and keep those whose
        // backing item is one of the subject's own items.
        let triples = store
            .list_triples(bank_id)?
            .into_iter()
            .filter(|triple| items.iter().any(|item| item.item_id == triple.item_id))
            .collect();
        let task_rows = store.list_tasks(bank_id, HISTORY_LIMIT)?;
        let tasks_capped = hit_history_limit(task_rows.len());
        let tasks = task_rows
            .into_iter()
            .filter(|task| task.subject_id == subject_id)
            .collect();
        let failure_rows = store.list_failures(bank_id, HISTORY_LIMIT)?;
        let failures_capped = hit_history_limit(failure_rows.len());
        let failures = failure_rows
            .into_iter()
            .filter(|failure| failure.subject_id == subject_id)
            .collect();
        let beliefs = store
            .list_beliefs(bank_id)?
            .into_iter()
            .filter(|belief| belief.subject_id == subject_id)
            .collect();
        Ok(Self {
            items,
            triples,
            tasks,
            failures,
            beliefs,
            tasks_capped,
            failures_capped,
        })
    }

    /// The subject's record ids for one family, or `None` when the store has
    /// no per-subject read for it. The flag marks a bank-wide read that
    /// returned its full row cap, so the count may be truncated.
    fn select(&self, family: &str) -> Option<(Vec<String>, bool)> {
        match family {
            "items" => Some((
                self.items.iter().map(|item| item.item_id.clone()).collect(),
                false,
            )),
            "triples" => Some((
                self.triples
                    .iter()
                    .map(|triple| triple.id.clone())
                    .collect(),
                false,
            )),
            "tasks" => Some((
                self.tasks.iter().map(|task| task.id.clone()).collect(),
                self.tasks_capped,
            )),
            "failures" => Some((
                self.failures
                    .iter()
                    .map(|failure| failure.id.clone())
                    .collect(),
                self.failures_capped,
            )),
            "beliefs" => Some((
                self.beliefs
                    .iter()
                    .map(|belief| belief.id.clone())
                    .collect(),
                false,
            )),
            _ => None,
        }
    }
}

/// The measured tables of one backend's facts.
fn facts_tables(facts: &InventoryFacts) -> &[(String, u64)] {
    match facts {
        InventoryFacts::Sqlite(state) => &state.table_counts,
        InventoryFacts::Postgres(state) => &state.table_counts,
    }
}

/// The store-wide measured rows of one family's planned tables, each counted
/// table at most once.
fn measured_rows(plan: &RowPlan, tables: &[(String, u64)]) -> u64 {
    tables
        .iter()
        .filter(|(name, _)| {
            plan.tables
                .iter()
                .any(|pattern| pattern_matches(pattern, name))
        })
        .map(|(_, count)| *count)
        .sum()
}

/// One planned family's selection: its class, the records its own stored link
/// selects, the store-wide rows the mapping did not select, and the named
/// omission when one applies.
fn family_selection(
    plan: &RowPlan,
    records: &SubjectRecords,
    tables: &[(String, u64)],
) -> FamilySelection {
    let measured = measured_rows(plan, tables);
    if plan.tables.is_empty() {
        return binding(
            plan,
            FamilyClass::NotStored,
            0,
            Vec::new(),
            0,
            Some(gap_or_source(plan, measured)),
        );
    }
    if plan.readability == Readability::Hashed {
        return binding(
            plan,
            FamilyClass::UnknownLink,
            0,
            Vec::new(),
            measured,
            Some(gap_or_source(plan, measured)),
        );
    }
    match records.select(plan.family) {
        Some((selected_ids, capped)) => {
            let selected = selected_ids.len() as u64;
            let omitted = measured.saturating_sub(selected);
            let mut reasons = Vec::new();
            if omitted > 0 {
                reasons.push(format!(
                    "store-wide {measured} {} rows measured: {selected} selected for this subject, {omitted} not selected by this mapping",
                    plan.family
                ));
            }
            let missing = missing_tables(plan, tables);
            if !missing.is_empty() {
                reasons.push(format!(
                    "the family's planned tables are not in the live store facts: {}",
                    missing.join(", ")
                ));
            }
            if capped {
                reasons.push(format!(
                    "bank-wide read capped at {HISTORY_LIMIT} rows: this family's count may be truncated"
                ));
            }
            binding(
                plan,
                FamilyClass::Selected,
                selected,
                selected_ids,
                omitted,
                (!reasons.is_empty()).then(|| reasons.join("; ")),
            )
        }
        None if plan.link.is_some() => binding(
            plan,
            FamilyClass::OtherOnly,
            0,
            Vec::new(),
            measured,
            Some(format!(
                "store-wide {measured} {} rows measured: no per-subject read path on this store",
                plan.family
            )),
        ),
        None => binding(
            plan,
            FamilyClass::InseparableShared,
            0,
            Vec::new(),
            measured,
            Some(gap_or_source(plan, measured)),
        ),
    }
}

/// One selection from its parts; `complete` is derived from the omission.
fn binding(
    plan: &RowPlan,
    class: FamilyClass,
    selected: u64,
    selected_ids: Vec<String>,
    omitted: u64,
    omission: Option<String>,
) -> FamilySelection {
    FamilySelection {
        family: plan.family,
        class,
        selected,
        selected_ids,
        omitted,
        complete: omission.is_none(),
        omission,
    }
}

/// The row's own named gap, or its source kind when the row names no gap.
/// The gap-based classes omit the measured count itself, so a positive
/// store-wide count is appended to say where that number was measured.
fn gap_or_source(plan: &RowPlan, measured: u64) -> String {
    let gap = if let Some(gap) = plan.gap {
        gap.to_owned()
    } else {
        match plan.source {
            RowSource::Tables => plan.tables.join(", "),
            RowSource::Fixed(text) => text.to_owned(),
            RowSource::StoreFile => "store file (a local path, not a stored record)".to_owned(),
            RowSource::KmsFile => "key file (a local path, not a stored record)".to_owned(),
            RowSource::BackupDir => "backup archives root (an operator-held location)".to_owned(),
            RowSource::Copies => "caller-declared copies held outside the store".to_owned(),
            RowSource::Peers => "peer enablement and acknowledgements (not stored)".to_owned(),
        }
    };
    if measured > 0 {
        format!("{gap} (store-wide {measured} rows measured)")
    } else {
        gap
    }
}

#[cfg(test)]
#[path = "inventory_map_tests.rs"]
mod inventory_map_tests;

#[cfg(test)]
#[path = "inventory_map_pg_tests.rs"]
mod inventory_map_pg_tests;
