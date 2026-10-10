// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-revision evidence a distilled unit
//! carries: one entry per claimed source, holding that source's id and its
//! last-change time, plus the `evidence_json` codec both backends store it
//! with.
//!
//! ## Owns
//! - `EvidenceEntry` and `RevisionEvidence`, the append-only per-revision list
//! - `evidence_from_items`, the one rule that turns a distilled cluster into
//!   entries (ids and timestamps only, deduplicated, ordered by id)
//! - The `evidence_json` encode/decode pair
//!
//! ## Does not own
//! - The record that carries the list (`distilled_unit`)
//! - Freshness and staleness (`distilled_freshness`)
//! - Backend SQL (`sqlite_distilled_unit` / `postgres_distilled_unit`)
//!
//! ## Boundary
//!
//! Changes to the evidence entry shape or the cluster-to-entries rule should
//! normally require changes here. Changes to the record or to freshness should
//! not require changes here.
//!
//! Keep this module focused on the evidence trail itself.

use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode, MemoryItem};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One source that supported one revision of a distilled unit.
///
/// Ids and timestamps only. A source's text is never evidence here: the honest
/// record of what supported a summary is membership, and storing a gist would
/// duplicate content the erase closure then has to chase. `as_of` is the
/// source's own `updated_at` when the write path stamped one and `None` when it
/// did not, so an absent stamp is visible instead of invented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEntry {
    /// Item id of the claimed source.
    pub source_id: String,
    /// The source's last-change time at the moment this revision was built.
    pub as_of: Option<String>,
}

impl EvidenceEntry {
    /// Build a validated entry.
    ///
    /// # Errors
    /// An empty `source_id`.
    pub fn new(
        source_id: impl Into<String>,
        as_of: Option<impl Into<String>>,
    ) -> Result<Self, AmError> {
        let entry = Self {
            source_id: source_id.into(),
            as_of: as_of.map(Into::into),
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Structural well-formedness.
    ///
    /// # Errors
    /// An empty `source_id`.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.source_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "evidence entry source_id is required",
            ));
        }
        Ok(())
    }
}

/// One revision's evidence list, kept in the order the revisions happened.
///
/// `entries` is the list that revision was derived from, so a later revision
/// never rewrites an earlier one and the proof count of a superseded revision
/// stays readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionEvidence {
    /// The revision number this list belongs to.
    pub revision: u32,
    /// When that revision was reconciled.
    pub reconciled_at: String,
    /// The claimed sources at that revision, ordered by id.
    pub entries: Vec<EvidenceEntry>,
}

impl RevisionEvidence {
    /// Structural well-formedness: a non-zero revision, a reconciliation time,
    /// entries that each validate, and no id repeated inside one revision.
    ///
    /// # Errors
    /// A zero revision, an empty `reconciled_at`, an invalid entry, or a
    /// duplicate `source_id`.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.revision == 0 {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "revision evidence requires a non-zero revision",
            ));
        }
        if self.reconciled_at.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "revision evidence requires a reconciliation time",
            ));
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !seen.insert(entry.source_id.as_str()) {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "revision evidence repeats source `{}` in one revision",
                        entry.source_id
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// The evidence list of one revision, deduplicated by source id and ordered by
/// it.
///
/// The one rule both write paths call, so a first distillation and a
/// reconciliation cannot produce differently shaped lists. Ordering by id is
/// what makes the proof count and any comparison of two lists deterministic
/// rather than dependent on the order the cluster scan happened to return.
pub fn evidence_from_items<'a, I>(items: I) -> Vec<EvidenceEntry>
where
    I: IntoIterator<Item = &'a MemoryItem>,
{
    let mut by_id: BTreeMap<String, EvidenceEntry> = BTreeMap::new();
    for item in items {
        if item.id.is_empty() {
            continue;
        }
        // First writer wins for a repeated id, so the list is a set of sources
        // and a cluster that names one twice cannot inflate a proof count.
        by_id
            .entry(item.id.clone())
            .or_insert_with(|| EvidenceEntry {
                source_id: item.id.clone(),
                as_of: (!item.updated_at.is_empty()).then(|| item.updated_at.clone()),
            });
    }
    by_id.into_values().collect()
}

/// Serialize the per-revision evidence list for the `evidence_json` column.
pub(crate) fn encode_evidence(list: &[RevisionEvidence]) -> String {
    serde_json::to_string(list).expect("distilled unit evidence serializes")
}

/// Parse the `evidence_json` column.
pub(crate) fn decode_evidence(raw: &str) -> Result<Vec<RevisionEvidence>, AmError> {
    serde_json::from_str(raw).am("distilled unit evidence")
}

#[cfg(test)]
#[path = "distilled_evidence_tests.rs"]
mod distilled_evidence_tests;
