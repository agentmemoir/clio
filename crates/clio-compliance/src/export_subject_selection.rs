// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one-subject export payload: the
//! subject's own records under the export's narrowing filters, collected from
//! the split repository views, plus the completeness manifest the delivered
//! file embeds.
//!
//! ## Owns
//! - `build_subject_selection`, the subject payload's bytes and manifest
//! - `SplitFamilyLinkRead`, the adapter from an export's four views to one
//!   `FamilyLinkRead`
//! - The subject-scoped link collection (triples, tasks, failures, beliefs)
//!
//! ## Does not own
//! - The item collection and masking helpers (`crate::export_subject`)
//! - The subject family selection and its named omissions
//!   (`crate::subject_bundle`)
//! - The export entrypoint and the delivered file write (`crate::export`)
//!
//! ## Boundary
//!
//! Changes to what a subject selection contains or which narrowing filters it
//! applies should normally require changes here. Changes to the bank-wide
//! collection or the export entrypoint should not.
//!
//! Keep this module focused on the subject export payload.

use crate::bundle::{
    BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, BundleEnvelope, CompletenessManifest, ExportEntities,
    entities_checksum,
};
use crate::export::{ExportArgs, HISTORY_LIMIT, Truncation, hit_history_limit};
use crate::export_mask::{mask_beliefs, mask_failures, mask_tasks, mask_triples};
use crate::export_subject::{ExportPayload, collect_items, counts_by_category};
use crate::inventory::inventory_rows::{
    FamilyLinkRead, SubjectBundle, SubjectFamilyInput, build_subject_bundle,
};
use clio_store::{BeliefStore, ComplianceStore, HistoryStore, ItemIdentity, Store};
use clio_types::{AmError, BeliefObject, ErrorCode, FailureRecord, TaskRecord, TripleRecord};
use std::collections::BTreeSet;

/// The four repository views one export call holds, adapted to a single
/// [`FamilyLinkRead`] so a subject bundle can be built without a change in
/// `clio-store`.
pub(crate) struct SplitFamilyLinkRead<'a> {
    /// Item / triple persistence boundary.
    pub(crate) store: &'a dyn Store,
    /// Subject-item reads.
    pub(crate) compliance: &'a dyn ComplianceStore,
    /// Task / failure history reads.
    pub(crate) history: &'a dyn HistoryStore,
    /// Belief object reads.
    pub(crate) belief: &'a dyn BeliefStore,
}

impl FamilyLinkRead for SplitFamilyLinkRead<'_> {
    fn list_subject_items(
        &self,
        bank_id: &str,
        subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, AmError> {
        self.compliance.list_subject_items(bank_id, subject_id)
    }

    fn list_triples(&self, bank_id: &str) -> Result<Vec<TripleRecord>, AmError> {
        self.store.list_triples(bank_id)
    }

    fn list_tasks(&self, bank_id: &str, limit: usize) -> Result<Vec<TaskRecord>, AmError> {
        self.history.list_tasks(bank_id, limit)
    }

    fn list_failures(&self, bank_id: &str, limit: usize) -> Result<Vec<FailureRecord>, AmError> {
        self.history.list_failures(bank_id, limit)
    }

    fn list_beliefs(&self, bank_id: &str) -> Result<Vec<BeliefObject>, AmError> {
        self.belief.list_beliefs(bank_id)
    }
}

/// Build one subject's export payload: the subject's records under the
/// export's narrowing filters, wrapped in the bank bundle envelope so the
/// delivered file carries the records plus their completeness manifest.
///
/// The family selection's named omissions and the export's own narrowing and
/// truncation both land in the manifest, so `complete` is true only when
/// nothing was omitted, narrowed, or truncated.
///
/// # Errors
/// A subject export without measured facts or a backend, storage read
/// failures, or an unbuildable bundle.
pub(crate) fn build_subject_selection(
    args: &ExportArgs<'_>,
    bank: &str,
    subject_id: &str,
) -> Result<ExportPayload, AmError> {
    let facts = args.facts.ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "subject export requires measured live-store facts",
        )
    })?;
    let backend = args.backend.ok_or_else(|| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "subject export requires a store backend",
        )
    })?;
    let split = SplitFamilyLinkRead {
        store: args.store,
        compliance: args.compliance,
        history: args.history,
        belief: args.belief,
    };
    let input = SubjectFamilyInput {
        store: &split,
        bank_id: bank,
        subject_id,
        facts,
        backend,
    };
    let bundle = build_subject_bundle(&input)?;
    let subject_items = args.compliance.list_subject_items(bank, subject_id)?;
    let item_ids: BTreeSet<String> = subject_items
        .iter()
        .map(|item| item.item_id.clone())
        .collect();
    let mut entities = ExportEntities::default();
    collect_items(args, bank, Some(&item_ids), &mut entities)?;
    let truncation = collect_subject_links(args, bank, subject_id, &item_ids, &mut entities)?;
    let manifest = subject_manifest(args, bank, subject_id, &bundle, &entities, truncation);
    let envelope = BundleEnvelope {
        format: BUNDLE_FORMAT.to_owned(),
        format_version: BUNDLE_FORMAT_VERSION,
        content_mode: args.content_mode,
        manifest: manifest.clone(),
        entities,
    };
    let text = serde_json::to_string_pretty(&envelope)
        .expect("bundle serialization cannot fail: every float is validated finite at the store");
    Ok(ExportPayload {
        bytes: text.into_bytes(),
        manifest,
    })
}

/// The subject's linked records: triples carried by the subject's own items,
/// plus the task, failure, and belief rows whose subject column names the
/// subject. Bank-wide reads are filtered before masking, so no other
/// subject's record enters the payload.
fn collect_subject_links(
    args: &ExportArgs<'_>,
    bank: &str,
    subject_id: &str,
    subject_items: &BTreeSet<String>,
    entities: &mut ExportEntities,
) -> Result<Truncation, AmError> {
    let mode = args.content_mode;
    entities.triples = args
        .store
        .list_triples(bank)?
        .into_iter()
        .filter(|triple| subject_items.contains(&triple.item_id))
        .collect();
    mask_triples(&mut entities.triples, mode);
    let mut task_rows = args.history.list_tasks(bank, HISTORY_LIMIT)?;
    let tasks = hit_history_limit(task_rows.len());
    task_rows.retain(|task| task.subject_id == subject_id);
    mask_tasks(&mut task_rows, mode);
    entities.tasks = task_rows;
    let mut failure_rows = args.history.list_failures(bank, HISTORY_LIMIT)?;
    let failures = hit_history_limit(failure_rows.len());
    failure_rows.retain(|failure| failure.subject_id == subject_id);
    mask_failures(&mut failure_rows, mode);
    entities.failures = failure_rows;
    let mut beliefs: Vec<BeliefObject> = args
        .belief
        .list_beliefs(bank)?
        .into_iter()
        .filter(|belief| belief.subject_id == subject_id)
        .collect();
    mask_beliefs(&mut beliefs, mode);
    entities.beliefs = beliefs;
    Ok(Truncation { tasks, failures })
}

/// The manifest for a subject selection: the counts the delivered entities
/// carry, the checksum over them, the scope actually selected, and every
/// named omission. The bank manifest's association-edge narrowing notes do
/// not apply: a subject selection carries triples, tasks, failures, and
/// beliefs, and no association edges at all.
fn subject_manifest(
    args: &ExportArgs<'_>,
    bank: &str,
    subject_id: &str,
    bundle: &SubjectBundle,
    entities: &ExportEntities,
    truncation: Truncation,
) -> CompletenessManifest {
    let mut omissions = bundle.manifest.omissions.clone();
    if args
        .filter
        .category
        .as_deref()
        .is_some_and(|c| !c.is_empty())
    {
        omissions.push("category filter narrowed items".to_owned());
    }
    if args.filter.time_start.is_some() || args.filter.time_end.is_some() {
        omissions.push("time range narrowed items".to_owned());
    }
    if !args.filter.include_discarded {
        omissions.push("discarded items excluded".to_owned());
    }
    if !args.filter.include_archived {
        omissions.push("archived items excluded".to_owned());
    }
    if truncation.tasks {
        omissions.push(format!("task history truncated at {HISTORY_LIMIT} rows"));
    }
    if truncation.failures {
        omissions.push(format!("failure history truncated at {HISTORY_LIMIT} rows"));
    }
    CompletenessManifest {
        format_version: BUNDLE_FORMAT_VERSION,
        content_mode: args.content_mode,
        bank_ids: vec![bank.to_owned()],
        counts_by_type: entities.counts_by_type(),
        counts_by_category: counts_by_category(entities, args.content_mode),
        content_sha256: entities_checksum(entities),
        filter: serde_json::json!({
            "category": args.filter.category,
            "time_start": args.filter.time_start,
            "time_end": args.filter.time_end,
            "include_archived": args.filter.include_archived,
            "include_discarded": args.filter.include_discarded,
            "subject_id": subject_id,
        }),
        generated_at: args.timestamp.to_owned(),
        complete: omissions.is_empty(),
        omissions,
    }
}

#[cfg(test)]
#[path = "export_subject_selection_tests.rs"]
mod export_subject_selection_tests;
