// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `reindex` operator surface: a public UX
//! over the derived-index rebuild API with dry-run costing and confirmation.
//!
//! ## Owns
//! - [`reindex`]: dry-run scope/cost report and confirmed live rebuild
//! - [`ReindexReport`] shape
//!
//! ## Does not own
//! - The rebuild sweep itself (`clio_index::rebuild`)
//! - Finding emission or repair planning (`verify` / `doctor` / `repair`)
//!
//! ## Boundary
//!
//! Changes to reindex UX policy (costing fields, confirmation) should normally
//! require changes here. Changes to sweep behavior should not require changes
//! here.
//!
//! A dense target needs an embedding sidecar; a lexical-only run works without
//! one. A live run always requires confirmation.

use clio_index::{Embedder, RebuildReport, RebuildTargets};
use clio_store::{IndexStore, Store};
use clio_types::{AmError, ErrorCode};
use serde::Serialize;

/// Inputs for [`reindex`].
pub struct ReindexArgs<'a> {
    /// Writer/reader used by the dense sweep (upsert vectors, read items).
    pub store: &'a dyn Store,
    /// Index backend view (coverage, queue, lexical docs).
    pub index: &'a dyn IndexStore,
    /// Embedding sidecar (required for a live dense rebuild).
    pub embedder: Option<&'a dyn Embedder>,
    /// Bank scope (`None` = every indexable bank).
    pub bank: Option<&'a str>,
    /// Which derived indexes to rebuild.
    pub target: RebuildTargets,
    /// Report scope/cost without writing.
    pub dry_run: bool,
    /// Required for a mutating run.
    pub confirm: bool,
}

/// Result of one `reindex` call (dry-run or live).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReindexReport {
    /// True only for a live apply. A dry run writes nothing, so a no-op report
    /// is never `ok`.
    pub ok: bool,
    /// Scope bank (`None` = whole store).
    pub bank: Option<String>,
    /// Target label (`dense` / `lexical` / `all`).
    pub target: String,
    /// Whether the run wrote anything.
    pub dry_run: bool,
    /// Open, non-discarded indexable items in scope.
    pub indexable_items: u64,
    /// Items missing a dense row before the sweep (cost estimate).
    pub dense_missing: u64,
    /// Items missing a lexical row before the sweep (cost estimate).
    pub lexical_missing: u64,
    /// Dense rows written (0 on dry-run).
    pub dense_applied: u64,
    /// Lexical rows written (0 on dry-run).
    pub lexical_applied: u64,
    /// Derived rows for vanished items removed (0 on dry-run).
    pub orphans_removed: u64,
}

/// Report scope/cost (`dry_run=true`) or rebuild derived indexes (confirmed).
///
/// # Errors
/// Missing/empty bank, refused confirmation, a contradictory dry-run plus
/// confirm, missing embedder for a dense target, embed failures, or storage
/// failures.
pub fn reindex(args: &ReindexArgs<'_>) -> Result<ReindexReport, AmError> {
    // A dry run reports and a confirmed run applies, so the pair contradicts.
    // Refuse it instead of letting the report path quietly apply nothing.
    if args.dry_run && args.confirm {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "confirm contradicts a dry run: a dry run applies nothing, so only \
             one of the two may be set",
        ));
    }
    let report: RebuildReport = clio_index::rebuild(
        args.store,
        args.index,
        args.embedder,
        args.bank,
        args.target,
        args.dry_run,
        args.confirm,
    )?;
    Ok(ReindexReport {
        ok: !report.dry_run,
        bank: report.bank.clone(),
        target: target_label(args.target).to_owned(),
        dry_run: report.dry_run,
        indexable_items: report.indexable_items,
        dense_missing: report.dense_missing,
        lexical_missing: report.lexical_missing,
        dense_applied: report.dense_applied,
        lexical_applied: report.lexical_applied,
        orphans_removed: report.orphans_removed,
    })
}

/// Wire label for a rebuild target.
#[must_use]
pub fn target_label(target: RebuildTargets) -> &'static str {
    match target {
        RebuildTargets::Dense => "dense",
        RebuildTargets::Lexical => "lexical",
        RebuildTargets::All => "all",
    }
}

#[cfg(test)]
#[path = "reindex_tests.rs"]
mod reindex_tests;

#[cfg(test)]
#[path = "reindex_space_tests.rs"]
mod reindex_space_tests;
