// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the backend-agnostic derived-index rebuild
//! sweep: dry-run costing plus confirmed dense/lexical re-derivation with
//! orphan cleanup.
//!
//! ## Owns
//! - `rebuild`: scope resolution, coverage costing, dense/lexical sweeps, and
//!   scope-honoring orphan cleanup over a `Store` + `IndexStore` pair
//!
//! ## Does not own
//! - Outbox processing or retry state (`worker`)
//! - Backend index SQL (`clio_store::IndexStore`)
//! - Embedding transport (`embed` / `http`)
//!
//! ## Boundary
//!
//! Changes to rebuild sweep policy should normally require changes here.
//! Changes to worker retry policy or backend SQL should not require changes here.
//!
//! The sweep is idempotent: dense vectors and lexical docs are derived state,
//! so re-deriving them is always safe. Dense re-derivation needs an embedder;
//! a lexical-only run works without one.

use crate::embed::{Embedder, embed_text, lexical_text};
use crate::report::{RebuildReport, RebuildTargets};
use clio_store::{IndexStore, Store, VectorSlot};
use clio_types::{AmError, ErrorCode};

/// Embed batch size used by rebuild sweeps.
pub const REBUILD_EMBED_BATCH: usize = 16;
/// Rebuild page size (visible items per sweep step).
pub const REBUILD_PAGE: usize = 200;

/// Rebuild derived indexes for `bank` (or every indexable bank when `None`).
///
/// `dry_run=true` reports scope and gap counts without writing. A mutating run
/// requires `confirm=true`. A dense target needs an embedder; when none is
/// configured the run fails closed rather than silently skipping dense rows.
/// Orphan cleanup runs once per call, restricted to `bank` when one is given.
///
/// # Errors
/// Missing/empty bank, refused confirmation, missing embedder for a dense
/// target, embed failures, or storage failures.
pub fn rebuild(
    store: &dyn Store,
    index: &dyn IndexStore,
    embedder: Option<&dyn Embedder>,
    bank: Option<&str>,
    targets: RebuildTargets,
    dry_run: bool,
    confirm: bool,
) -> Result<RebuildReport, AmError> {
    if let Some(b) = bank
        && b.is_empty()
    {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if !dry_run && !confirm {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "mutating rebuild requires confirm=true (or dry_run=true for a report)",
        ));
    }
    let wants_dense = matches!(targets, RebuildTargets::Dense | RebuildTargets::All);
    if !dry_run && wants_dense && embedder.is_none() {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "dense rebuild requires an embedding sidecar (set embed.url); \
             run a lexical-only rebuild without it",
        ));
    }
    let banks: Vec<String> = match bank {
        Some(b) => vec![b.to_owned()],
        None => index.indexable_banks()?,
    };
    let mut report = RebuildReport {
        bank: bank.map(str::to_owned),
        dry_run,
        ..RebuildReport::default()
    };
    for scope in &banks {
        let coverage = index.index_coverage(Some(scope))?;
        report.indexable_items += coverage.indexable_items;
        report.dense_missing += coverage.indexable_items.saturating_sub(coverage.dense_rows);
        report.lexical_missing += coverage
            .indexable_items
            .saturating_sub(coverage.lexical_rows);
        if dry_run {
            continue;
        }
        if wants_dense && let Some(embedder) = embedder {
            report.dense_applied += sweep_dense(store, index, embedder, scope)?;
        }
        if matches!(targets, RebuildTargets::Lexical | RebuildTargets::All) {
            report.lexical_applied += sweep_lexical(store, index, scope)?;
        }
    }
    // Orphan cleanup runs once per call and never exceeds the requested scope:
    // a bank-scoped reindex must not delete derived rows of other banks, and a
    // whole-store run sweeps globally exactly once.
    if !dry_run {
        report.orphans_removed = index.cleanup_orphans(bank)?;
    }
    Ok(report)
}

/// Re-derive dense vectors + lexical docs for every visible indexable item.
fn sweep_dense(
    store: &dyn Store,
    index: &dyn IndexStore,
    embedder: &dyn Embedder,
    bank: &str,
) -> Result<u64, AmError> {
    let mut applied = 0_u64;
    for_each_visible_page(index, bank, |ids| {
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(item) = fetch_indexable_item(store, bank, id)?
                && let Some(text) = embed_text(&item)
            {
                pairs.push((id.clone(), text));
            }
        }
        for chunk in pairs.chunks(REBUILD_EMBED_BATCH) {
            let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
            let vectors = embed_and_validate(embedder, &texts)?;
            for ((id, _), vector) in chunk.iter().zip(vectors) {
                store.upsert_vector(&VectorSlot {
                    item_id: id.clone(),
                    bank_id: bank.to_owned(),
                    model_id: embedder.model_id(),
                    embedding: vector,
                })?;
                index.finish_index_job(bank, id)?;
                applied += 1;
            }
        }
        Ok(())
    })?;
    Ok(applied)
}

/// Re-derive lexical docs for every visible indexable item.
fn sweep_lexical(store: &dyn Store, index: &dyn IndexStore, bank: &str) -> Result<u64, AmError> {
    let mut applied = 0_u64;
    for_each_visible_page(index, bank, |ids| {
        for id in ids {
            if let Some(item) = fetch_indexable_item(store, bank, id)?
                && let Some(text) = lexical_text(&item)
            {
                index.put_lexical_doc(bank, id, &text)?;
                index.finish_index_job(bank, id)?;
                applied += 1;
            }
        }
        Ok(())
    })?;
    Ok(applied)
}

/// Fetch one item for indexing: `None` when gone, invalidated, or unreadable
/// (nothing to derive from).
///
/// An unreadable row is skipped rather than failing the rebuild. That covers a
/// legacy opaque envelope, a content version this build cannot read, a damaged
/// ciphertext, and a crypto-shredded subject. A broken backend still errors.
fn fetch_indexable_item(
    store: &dyn Store,
    bank: &str,
    id: &str,
) -> Result<Option<clio_types::MemoryItem>, AmError> {
    match store.get_memory_item(bank, id) {
        Ok(item) => Ok(item),
        Err(e) if clio_store::is_unreadable_content(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Embed one batch, failing closed on any dimension drift before storage sees it.
fn embed_and_validate(embedder: &dyn Embedder, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
    let vectors = embedder.embed_batch(texts)?;
    for (i, vector) in vectors.iter().enumerate() {
        if vector.len() != embedder.dims() {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "embed model {} returned {} dims for input {i}; expected {} \
                     (dim mismatch fails closed; re-configure embed.dims and rebuild)",
                    embedder.model_id(),
                    vector.len(),
                    embedder.dims()
                ),
            ));
        }
    }
    Ok(vectors)
}

/// Invoke `f` once per page of visible indexable item IDs, fetching pages
/// lazily so a rebuild sweep never buffers a whole bank in memory.
fn for_each_visible_page(
    index: &dyn IndexStore,
    bank: &str,
    mut f: impl FnMut(&[String]) -> Result<(), AmError>,
) -> Result<(), AmError> {
    let total = index.count_indexable_items(bank)?;
    let mut offset = 0_usize;
    while offset < total as usize {
        let ids = index.indexable_items(bank, REBUILD_PAGE, offset)?;
        if ids.is_empty() {
            break;
        }
        offset += ids.len();
        f(&ids)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "rebuild_tests.rs"]
mod rebuild_tests;

#[cfg(test)]
#[path = "rebuild_embed_tests.rs"]
mod rebuild_embed_tests;
