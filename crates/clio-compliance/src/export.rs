// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the single-file JSON memory export writer
//! (`memory_export_bundle` + `completeness_manifest`):
//! the authorization gate, item enumeration, bank-wide entity collection,
//! defensive secret masking, and the completeness manifest.
//!
//! A DEK-less `ciphertext_backup` cannot carry readable record content, so its
//! domain records keep only their metadata inventory (`crate::export_mask`).
//!
//! ## Owns
//! - `export` entrypoint, the harness/system authorization gate, and the
//!   fail-closed write (no partial destination file)
//! - Item enumeration over the inspector filter, content-mode aware
//! - Bank-wide entity collection with defensive masking delegated to
//!   `crate::export_mask`
//! - `CompletenessManifest` derivation from the collected payload
//!
//! ## Does not own
//! - The bundle file contract, canonicalization, and checksums (`crate::bundle`)
//! - Backend SQL (`clio-store`) or MCP argument decoding (`clio-mcp`)
//! - The restore/import path
//!
//! ## Boundary
//!
//! Changes to how entities are enumerated, masked, or how the manifest is
//! derived should normally require changes here. Changes to the bundle file
//! layout or checksums should not require changes here.
//!
//! Keep this module focused on the export writer.

use crate::bundle::{
    BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, BundleEnvelope, CompletenessManifest, ExportContentMode,
    ExportEntities, OpaqueItem, entities_checksum,
};
use crate::export_mask::{
    mask_assoc_edges, mask_beliefs, mask_failures, mask_item, mask_preference, mask_stable,
    mask_tasks, mask_triples,
};
use clio_store::{
    BeliefStore, ComplianceStore, HistoryStore, InspectValidity, ItemInspectFilter, ItemInspectRow,
    PersonaStore, Store,
};
use clio_types::{Actor, AmError, ErrorCode, ItemKind, OpContext};
use std::collections::{BTreeMap, BTreeSet};

/// Deterministic inspector page size.
const PAGE: usize = 200;

/// Upper bound for bank-wide history reads.
pub(crate) const HISTORY_LIMIT: usize = 10_000;

/// Which bounded history reads returned their full row cap (may be truncated).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Truncation {
    /// Task listing reached [`HISTORY_LIMIT`].
    pub(crate) tasks: bool,
    /// Failure listing reached [`HISTORY_LIMIT`].
    pub(crate) failures: bool,
}

/// True when a bounded history read returned the full row cap.
#[must_use]
pub(crate) fn hit_history_limit(rows: usize) -> bool {
    rows >= HISTORY_LIMIT
}

/// Caller-selectable export narrowing; defaults export the whole bank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFilter {
    /// Semantic category filter; `None` or `""` exports all categories.
    pub category: Option<String>,
    /// `created_at` lower bound (inclusive).
    pub time_start: Option<String>,
    /// `created_at` upper bound (exclusive).
    pub time_end: Option<String>,
    /// Include hygiene-archived rows.
    pub include_archived: bool,
    /// Include operationally discarded rows.
    pub include_discarded: bool,
}

impl Default for ExportFilter {
    /// Everything included: the only fully complete selection.
    fn default() -> Self {
        Self {
            category: None,
            time_start: None,
            time_end: None,
            include_archived: true,
            include_discarded: true,
        }
    }
}

/// Inputs for the in-process memory export tool.
pub struct ExportArgs<'a> {
    /// Item / triple / assoc persistence boundary.
    pub store: &'a dyn Store,
    /// Tombstone reads.
    pub compliance: &'a dyn ComplianceStore,
    /// Persona document reads.
    pub persona: &'a dyn PersonaStore,
    /// Task / failure history reads.
    pub history: &'a dyn HistoryStore,
    /// Belief object reads.
    pub belief: &'a dyn BeliefStore,
    /// Bank / actor context (actor must be privileged).
    pub ctx: &'a OpContext,
    /// Destination file path.
    pub destination: &'a str,
    /// Selection narrowing; non-default filters mark the manifest incomplete.
    pub filter: ExportFilter,
    /// Content mode of the payload.
    pub content_mode: ExportContentMode,
    /// RFC 3339 UTC timestamp recorded as `generated_at`.
    pub timestamp: &'a str,
}

/// Export result.
#[derive(Debug, Clone)]
pub struct ExportOutcome {
    /// Destination path written.
    pub destination: String,
    /// Completeness manifest embedded in the file.
    pub manifest: CompletenessManifest,
    /// Content mode of the payload.
    pub content_mode: ExportContentMode,
}

/// In-process memory export: one `memory_export_bundle` file plus its
/// completeness manifest.
///
/// AUTHORISATION: only `harness` or `system` actors may export; agent/user
/// callers get [`ErrorCode::Forbidden`] and no file is created. The file is
/// written once, fully built in memory, so no partial file can exist.
///
/// # Errors
/// Authorization, missing bank, storage, or write failures.
pub fn export(args: &ExportArgs<'_>) -> Result<ExportOutcome, AmError> {
    let bank = args.ctx.bank.as_str();
    if bank.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    authorize(args.ctx.actor)?;
    let mut entities = ExportEntities::default();
    collect_items(args, bank, &mut entities)?;
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
    std::fs::write(args.destination, text)
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("bundle write failed: {e}")))?;
    Ok(ExportOutcome {
        destination: args.destination.to_owned(),
        manifest,
        content_mode: args.content_mode,
    })
}

/// Only harness/system actors carry export privilege.
fn authorize(actor: Actor) -> Result<(), AmError> {
    match actor {
        Actor::Harness | Actor::System => Ok(()),
        Actor::Agent | Actor::User => Err(AmError::new(
            ErrorCode::Forbidden,
            "memory export requires a harness or system actor with compliance privilege",
        )),
    }
}

/// Page through the inspector and collect items in the requested content mode.
fn collect_items(
    args: &ExportArgs<'_>,
    bank: &str,
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
fn counts_by_category(entities: &ExportEntities, mode: ExportContentMode) -> Vec<(String, usize)> {
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

/// Honest completeness: `true` only when nothing was narrowed or truncated.
/// Any narrowing makes other bank-wide entities unfiltered relative to items.
pub(crate) fn completeness(filter: &ExportFilter, truncation: Truncation) -> (bool, Vec<String>) {
    let mut omissions = Vec::new();
    if filter.category.as_deref().is_some_and(|c| !c.is_empty()) {
        omissions.push("category filter narrowed items".to_owned());
    }
    if filter.time_start.is_some() || filter.time_end.is_some() {
        omissions.push("time range narrowed items".to_owned());
    }
    if !filter.include_discarded {
        omissions.push("discarded items excluded".to_owned());
    }
    if !filter.include_archived {
        omissions.push("archived items excluded".to_owned());
    }
    if !omissions.is_empty() {
        omissions.push("assoc edges exported unfiltered".to_owned());
    }
    if truncation.tasks {
        omissions.push(format!("task history truncated at {HISTORY_LIMIT} rows"));
    }
    if truncation.failures {
        omissions.push(format!("failure history truncated at {HISTORY_LIMIT} rows"));
    }
    (omissions.is_empty(), omissions)
}

#[cfg(test)]
#[path = "export_completeness_tests.rs"]
mod export_completeness_tests;

#[cfg(test)]
#[path = "export_context_tests.rs"]
mod export_context_tests;

#[cfg(test)]
#[path = "export_security_tests.rs"]
mod export_security_tests;

#[cfg(test)]
#[path = "export_truncation_tests.rs"]
mod export_truncation_tests;
