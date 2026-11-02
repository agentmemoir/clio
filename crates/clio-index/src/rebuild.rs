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
use crate::report::{RebuildReport, RebuildTargets, RowLoss, classify_row_loss};
use clio_store::{IndexStore, Store, VectorSlot};
use clio_types::{AmError, ErrorCode};

/// Embed batch size used by rebuild sweeps.
pub const REBUILD_EMBED_BATCH: usize = 16;
/// Rebuild page size (visible items per sweep step).
pub const REBUILD_PAGE: usize = 200;

/// What one visible item yielded for a sweep.
#[derive(Debug)]
pub(crate) enum Fetched {
    /// The item opened; derive from it.
    Item(Box<clio_types::MemoryItem>),
    /// No row: gone or invalidated while the sweep ran.
    Gone,
    /// The row exists but its content cannot be opened.
    Lost(RowLoss),
}

/// Row-loss totals for one sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RowCounts {
    pub(crate) unreadable: u64,
    pub(crate) erased: u64,
}

impl RowCounts {
    /// Count one lost row.
    pub(crate) fn record(&mut self, loss: RowLoss) {
        match loss {
            RowLoss::Unreadable => self.unreadable += 1,
            RowLoss::Erased => self.erased += 1,
        }
    }
}

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
        let mut counts = None;
        if wants_dense && let Some(embedder) = embedder {
            let (applied, swept) = sweep_dense(store, index, embedder, scope)?;
            report.dense_applied += applied;
            counts = Some(swept);
        }
        if matches!(targets, RebuildTargets::Lexical | RebuildTargets::All) {
            let (applied, swept) = sweep_lexical(store, index, scope)?;
            report.lexical_applied += applied;
            // Both sweeps visit the same rows, so a loss is counted once.
            counts = Some(swept);
        }
        if let Some(swept) = counts {
            report.unreadable += swept.unreadable;
            report.erased += swept.erased;
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
) -> Result<(u64, RowCounts), AmError> {
    let mut applied = 0_u64;
    let mut counts = RowCounts::default();
    for_each_visible_page(index, bank, |ids| {
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(ids.len());
        for id in ids {
            match fetch_indexable_item(store, bank, id)? {
                Fetched::Item(item) => {
                    if let Some(text) = embed_text(&item) {
                        pairs.push((id.clone(), text));
                    }
                }
                Fetched::Gone => {}
                Fetched::Lost(loss) => counts.record(loss),
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
    Ok((applied, counts))
}

/// Re-derive lexical docs for every visible indexable item.
fn sweep_lexical(
    store: &dyn Store,
    index: &dyn IndexStore,
    bank: &str,
) -> Result<(u64, RowCounts), AmError> {
    let mut applied = 0_u64;
    let mut counts = RowCounts::default();
    for_each_visible_page(index, bank, |ids| {
        for id in ids {
            match fetch_indexable_item(store, bank, id)? {
                Fetched::Item(item) => {
                    if let Some(text) = lexical_text(&item) {
                        index.put_lexical_doc(bank, id, &text)?;
                        index.finish_index_job(bank, id)?;
                        applied += 1;
                    }
                }
                Fetched::Gone => {}
                Fetched::Lost(loss) => counts.record(loss),
            }
        }
        Ok(())
    })?;
    Ok((applied, counts))
}

/// Fetch one item for indexing: `Gone` when it vanished, `Lost` when its
/// content cannot be opened, and the item itself when it opened.
///
/// A lost row is counted, never retried and never fatal. That covers a legacy
/// opaque envelope, a content version this build cannot read, a damaged
/// ciphertext, and a crypto-shredded subject. A broken backend still errors.
pub(crate) fn fetch_indexable_item(
    store: &dyn Store,
    bank: &str,
    id: &str,
) -> Result<Fetched, AmError> {
    match store.get_memory_item(bank, id) {
        Ok(Some(item)) => Ok(Fetched::Item(Box::new(item))),
        Ok(None) => Ok(Fetched::Gone),
        Err(e) => match classify_row_loss(&e) {
            Some(loss) => Ok(Fetched::Lost(loss)),
            None => Err(e),
        },
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
