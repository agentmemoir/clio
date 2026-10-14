// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for building an export payload: the whole-bank
//! item and entity collection with defensive masking, the completeness
//! manifest, and the dispatch between the bank payload and the one-subject
//! selection payload.
//!
//! ## Owns
//! - `build_payload` and `ExportPayload`, the bytes-plus-manifest result
//! - The bank-wide collection helpers (items, triples, assoc edges, persona,
//!   tasks, failures, beliefs, tombstones) and their masking
//!
//! ## Does not own
//! - The one-subject selection payload (`crate::export_subject_selection`)
//! - The subject family selection and its named omissions
//!   (`crate::subject_bundle`)
//! - The export entrypoint, authorization, and the delivery-or-write choice
//!   (`crate::export`)
//! - The bundle file contract and checksums (`crate::bundle`)
//! - The subject-family mapping itself (`crate::inventory`)
//!
//! ## Boundary
//!
//! Changes to what a bank payload contains or how it is collected should
//! normally require changes here. Changes to the subject payload, the export
//! entrypoint, or the bundle layout should not.
//!
//! Keep this module focused on building bank export payloads.

#[path = "export_subject_selection.rs"]
pub(crate) mod export_subject_selection;

use crate::bundle::{
    BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, BundleEnvelope, CompletenessManifest, ExportContentMode,
    ExportEntities, OpaqueItem, entities_checksum,
};
use crate::export::{ExportArgs, HISTORY_LIMIT, Truncation, completeness, hit_history_limit};
use crate::export_mask::{
    mask_assoc_edges, mask_beliefs, mask_failures, mask_item, mask_preference, mask_stable,
    mask_tasks, mask_triples,
};
use clio_store::{InspectValidity, ItemInspectFilter, ItemInspectRow};
use clio_types::{AmError, ErrorCode, ItemKind};
use std::collections::{BTreeMap, BTreeSet};

/// Deterministic inspector page size.
const PAGE: usize = 200;

/// One built export payload: the bytes to write plus its manifest.
pub(crate) struct ExportPayload {
    /// The fully built payload bytes.
    pub(crate) bytes: Vec<u8>,
    /// The completeness manifest describing the payload.
    pub(crate) manifest: CompletenessManifest,
}

/// Build the payload for `args`: one subject's selection when a subject is
/// selected, otherwise the whole-bank bundle.
///
/// # Errors
/// A subject export without measured facts or a backend, storage read
/// failures, or an unbuildable bundle.
pub(crate) fn build_payload(args: &ExportArgs<'_>, bank: &str) -> Result<ExportPayload, AmError> {
    match args.subject_id.or(args.filter.subject_id.as_deref()) {
        Some(subject_id) => {
            export_subject_selection::build_subject_selection(args, bank, subject_id)
        }
        None => build_bank_payload(args, bank),
    }
}

/// The whole-bank bundle: every collected entity plus its completeness manifest.
fn build_bank_payload(args: &ExportArgs<'_>, bank: &str) -> Result<ExportPayload, AmError> {
    let mut entities = ExportEntities::default();
    collect_items(args, bank, None, &mut entities)?;
    let truncation = collect_bank_entities(args, bank, &mut entities)?;
    let manifest = build_manifest(args, bank, &entities, truncation);
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

/// Page through the inspector and collect items in the requested content mode.
/// With `subject` set, only the subject's own items are collected, so the
/// selection stays subject-scoped under the same narrowing filters.
pub(crate) fn collect_items(
    args: &ExportArgs<'_>,
    bank: &str,
    subject: Option<&BTreeSet<String>>,
    entities: &mut ExportEntities,
) -> Result<(), AmError> {
    let mut filter = ItemInspectFilter::new(bank);
    filter.category = args
        .filter
        .category
        .as_deref()
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    filter.time_start.clone_from(&args.filter.time_start);
    filter.time_end.clone_from(&args.filter.time_end);
    filter.validity = InspectValidity::All;
    filter.include_discarded = args.filter.include_discarded;
    filter.limit = PAGE;
    let total = args.store.count_items(&filter)?;
    let mut offset = 0;
    while offset < total {
        filter.offset = offset;
        for row in args.store.inspect_items(&filter)? {
            if let Some(item_ids) = subject
                && !item_ids.contains(&row.item_id)
            {
                continue;
            }
            if row.archived && !args.filter.include_archived {
                continue;
            }
            match args.content_mode {
                ExportContentMode::DsarPlaintext => {
                    match args.store.get_memory_item(bank, &row.item_id) {
                        Ok(Some(item)) => entities.items.push(mask_item(item)),
                        Ok(None) => return Err(vanished(&row.item_id)),
                        Err(err) if is_shredded(&err) => {}
                        Err(err) => return Err(err),
                    }
                }
                ExportContentMode::CiphertextBackup => {
                    if let Some(opaque) = opaque_item(args, bank, &row)? {
                        entities.opaque_items.push(opaque);
                    }
                }
            }
        }
        offset += PAGE;
    }
    Ok(())
}

/// Compliance-shredded rows (subject DEK destroyed) are skipped: their content
/// is unrecoverable by design and the tombstone metadata attests the erasure.
fn is_shredded(err: &AmError) -> bool {
    err.code == ErrorCode::ErasedSubject
}

fn vanished(id: &str) -> AmError {
    AmError::new(
        ErrorCode::NotFound,
        format!("export: item `{id}` vanished during enumeration"),
    )
}

/// Build one opaque row from inspect metadata plus raw at-rest ciphertext.
///
/// Metadata stays complete the content body is the at-rest ciphertext
/// with no DEK material. `get_memory_item` supplies only the subject id; its
/// decrypted content halves are discarded. Returns `Ok(None)` for a
/// compliance-shredded row: erased content must not reappear in a bundle.
fn opaque_item(
    args: &ExportArgs<'_>,
    bank: &str,
    row: &ItemInspectRow,
) -> Result<Option<OpaqueItem>, AmError> {
    let meta = match args.store.get_memory_item(bank, &row.item_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => return Err(vanished(&row.item_id)),
        Err(err) if is_shredded(&err) => return Ok(None),
        Err(err) => return Err(err),
    };
    let content_cipher = args
        .store
        .inspect_content_ciphertext(bank, &row.item_id)?
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::NotFound,
                format!("export: no content ciphertext for item `{}`", row.item_id),
            )
        })?;
    Ok(Some(OpaqueItem {
        id: row.item_id.clone(),
        subject_id: meta.subject_id,
        kind: row.kind.clone(),
        category: row.category.clone(),
        episodic_type: row.episodic_type.clone(),
        epistemic_kind: row.epistemic_kind.clone(),
        source_type: row.source_type.clone(),
        confidence: row.confidence,
        admission_score: row.admission_score,
        source_ref: row.source_ref.clone(),
        valid_from: row.valid_from.clone(),
        valid_until: row.valid_until.clone(),
        tx_from: row.tx_from.clone(),
        tx_until: row.tx_until.clone(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        content_cipher,
    }))
}

/// Always-exported bank-wide entities (unfiltered, which is why any narrowing
/// makes the manifest incomplete). Domain-record prose and `context` fields are
/// masked defensively by [`crate::export_mask`] in `dsar_plaintext` mode, and
/// emitted content-free in `ciphertext_backup` mode.
fn collect_bank_entities(
    args: &ExportArgs<'_>,
    bank: &str,
    entities: &mut ExportEntities,
) -> Result<Truncation, AmError> {
    let mode = args.content_mode;
    entities.triples = args.store.list_triples(bank)?;
    mask_triples(&mut entities.triples, mode);
    entities.assoc_edges = args.store.list_assoc_edges(bank, None)?;
    mask_assoc_edges(&mut entities.assoc_edges, mode);
    collect_persona(args, bank, entities)?;
    let mut task_rows = args.history.list_tasks(bank, HISTORY_LIMIT)?;
    let tasks = hit_history_limit(task_rows.len());
    mask_tasks(&mut task_rows, mode);
    entities.tasks = task_rows;
    let mut failure_rows = args.history.list_failures(bank, HISTORY_LIMIT)?;
    let failures = hit_history_limit(failure_rows.len());
    mask_failures(&mut failure_rows, mode);
    entities.failures = failure_rows;
    let mut beliefs = args.belief.list_beliefs(bank)?;
    mask_beliefs(&mut beliefs, mode);
    entities.beliefs = beliefs;
    entities.tombstones = args.compliance.list_tombstones(bank)?;
    Ok(Truncation { tasks, failures })
}

/// Persona stable history (all versions, deduped) plus preference states.
/// Every version's value and `context` are masked before the dedup identity
/// is computed, so rows that differ only in secret content still dedupe.
fn collect_persona(
    args: &ExportArgs<'_>,
    bank: &str,
    entities: &mut ExportEntities,
) -> Result<(), AmError> {
    let mode = args.content_mode;
    let doc = args.persona.get_persona_document(bank)?;
    let mut seen: BTreeSet<(String, String, String, Option<String>)> = BTreeSet::new();
    for entry in &doc.stable {
        for mut version in args.persona.stable_history(bank, &entry.key)? {
            mask_stable(&mut version, mode);
            let identity = (
                version.key.clone(),
                version.as_of.clone(),
                version.value.clone(),
                version.context.clone(),
            );
            if seen.insert(identity) {
                entities.persona_stable.push(version);
            }
        }
    }
    entities.persona_preferences = doc.preferences;
    for pref in &mut entities.persona_preferences {
        mask_preference(pref, mode);
    }
    Ok(())
}

/// Manifest from the sorted payload: checksum, counts, filter echo, and
/// completeness (true only for a fully unfiltered, untruncated selection).
fn build_manifest(
    args: &ExportArgs<'_>,
    bank: &str,
    entities: &ExportEntities,
    truncation: Truncation,
) -> CompletenessManifest {
    let (complete, omissions) = completeness(&args.filter, truncation);
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
        }),
        generated_at: args.timestamp.to_owned(),
        complete,
        omissions,
    }
}

/// Count semantic items by category and episodic items by episodic type tag.
pub(crate) fn counts_by_category(
    entities: &ExportEntities,
    mode: ExportContentMode,
) -> Vec<(String, usize)> {
    let rows: Vec<(bool, Option<String>, Option<String>)> = match mode {
        ExportContentMode::DsarPlaintext => entities
            .items
            .iter()
            .map(|i| {
                (
                    i.kind == ItemKind::Semantic,
                    i.category.map(|c| c.as_str().to_owned()),
                    i.episodic_type.map(|t| t.as_str().to_owned()),
                )
            })
            .collect(),
        ExportContentMode::CiphertextBackup => entities
            .opaque_items
            .iter()
            .map(|i| {
                (
                    i.kind == "semantic",
                    i.category.clone(),
                    i.episodic_type.clone(),
                )
            })
            .collect(),
    };
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (semantic, category, episodic) in rows {
        let label = if semantic {
            category.unwrap_or_default()
        } else {
            match episodic {
                Some(tag) => tag,
                None => continue,
            }
        };
        *counts.entry(label).or_default() += 1;
    }
    counts.into_iter().collect()
}

#[cfg(test)]
#[path = "export_subject_scenarios_tests.rs"]
pub(crate) mod export_subject_scenarios_tests;

#[cfg(test)]
#[path = "export_subject_scenarios_pg_tests.rs"]
mod export_subject_scenarios_pg_tests;
