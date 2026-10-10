// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the page-local duplicate suppression that runs
//! inside read-time recall narrowing.
//!
//! ## Owns
//! - The exact-duplicate floor every page enforces, at every tolerance level
//! - The widened near-duplicate cap a `duplicate_tolerance_read` level asks for
//! - Normalized-text and token-set comparison shared by both
//!
//! ## Does not own
//! - Scope filtering, prefer-consolidated supersede, or backfill (see `dedup`)
//! - Ranking, fusion, or rerank (`hybrid` / `fusion` / `rerank`)
//!
//! ## Boundary
//!
//! Changes to what counts as a duplicate on a page should normally require
//! changes here. Changes to the counters or the report belong in `dedup`.
//!
//! Keep this module focused on page-local duplicate suppression.

use super::consolidated_source_ids;
use super::{DedupConfig, FusedHit, is_consolidated_item, is_stale};
use crate::finalize::select_page;
use crate::hybrid_util::doc_text;
use clio_types::MemoryItem;
use std::collections::{BTreeMap, BTreeSet};

/// Similarity cap that collapses only byte-for-byte identical token sets.
///
/// A Jaccard score of `1.0` means the two token sets are equal, so this cap
/// removes exact duplicates and nothing else. It is the floor the page always
/// enforces, so an exact duplicate can never reach a caller no matter how the
/// bank's read tolerance is configured.
pub(crate) const EXACT_DUPLICATE_CAP: f64 = 1.0;

/// The cap this call enforces: the configured tolerance level when the read cap
/// is active, otherwise exact-duplicate collapse only.
pub(crate) fn effective_cap(cfg: DedupConfig) -> f64 {
    if cfg.read_cap_active() {
        cfg.tolerance.read_similarity_cap()
    } else {
        EXACT_DUPLICATE_CAP
    }
}

/// Drop page items that the prefer-consolidated rule or the near-duplicate cap
/// rejects; returns true when at least one item was dropped.
/// Parameters of one page-aware narrowing pass.
pub(crate) struct NarrowPass {
    pub(crate) prefer_consolidated: bool,
    /// Similarity cap for this pass; always at least `EXACT_DUPLICATE_CAP`.
    pub(crate) cap: f64,
    pub(crate) limit: usize,
    pub(crate) budget: Option<usize>,
}

pub(crate) fn narrow_page(
    filtered: &mut Vec<FusedHit>,
    items: &BTreeMap<String, MemoryItem>,
    pass: &NarrowPass,
    suppressed_raw_ids: &mut Vec<String>,
    near_dup_suppressed: &mut usize,
    exact_dup_suppressed: &mut usize,
) -> bool {
    let page_ids: BTreeSet<String> = select_page(filtered, items, pass.limit, pass.budget)
        .into_iter()
        .collect();
    let page: Vec<FusedHit> = filtered
        .iter()
        .filter(|hit| page_ids.contains(&hit.id))
        .cloned()
        .collect();
    let mut dropped: BTreeSet<String> = BTreeSet::new();
    if pass.prefer_consolidated {
        let mut claimed = BTreeSet::new();
        for hit in &page {
            if let Some(item) = items.get(&hit.id).filter(|item| {
                // Only a CURRENT unit suppresses its raw sources. A stale one
                // may no longer represent the item it claims, so the raw items
                // stay visible whichever reason made it stale.
                is_consolidated_item(item) && !is_stale(item)
            }) {
                claimed.extend(consolidated_source_ids(item));
            }
        }
        for hit in &page {
            let is_parent = items.get(&hit.id).is_some_and(is_consolidated_item);
            if !is_parent && claimed.contains(&hit.id) {
                suppressed_raw_ids.push(hit.id.clone());
                dropped.insert(hit.id.clone());
            }
        }
    }
    {
        let cap_page: Vec<FusedHit> = page
            .iter()
            .filter(|hit| !dropped.contains(&hit.id))
            .cloned()
            .collect();
        let outcome = cap_near_duplicates(cap_page.clone(), items, pass.cap);
        *exact_dup_suppressed += outcome.exact_dropped;
        *near_dup_suppressed += outcome.near_dropped;
        for hit in &cap_page {
            if !outcome.kept_ids.contains(&hit.id) {
                dropped.insert(hit.id.clone());
            }
        }
    }
    if dropped.is_empty() {
        return false;
    }
    filtered.retain(|hit| !dropped.contains(&hit.id));
    true
}

/// Keep the higher-ranked representative of each duplicate page group.
///
/// An exact duplicate is two items whose normalized text is equal, and that
/// test always runs: it is what keeps a page from repeating one sentence, and it
/// is the same rule the write path uses, so a row the write path refused is
/// never one this drops for a different reason. The Jaccard cap only widens that
/// to near-duplicates, and only when a tolerance level asks for it; at
/// `EXACT_DUPLICATE_CAP` it is skipped so word order stays significant.
fn cap_near_duplicates(
    candidates: Vec<FusedHit>,
    items: &BTreeMap<String, MemoryItem>,
    cap: f64,
) -> CapOutcome {
    let near_duplicates = cap < EXACT_DUPLICATE_CAP;
    let mut kept_ids = BTreeSet::new();
    let mut kept_tokens: Vec<BTreeSet<String>> = Vec::new();
    let mut kept_exact: Vec<String> = Vec::new();
    let mut exact_dropped = 0_usize;
    let mut near_dropped = 0_usize;
    for hit in candidates {
        let text = items
            .get(&hit.id)
            .map(|item| doc_text(Some(item)))
            .unwrap_or_default();
        let exact = normalize_exact(&text);
        let tokens = token_set(&text);
        // Exact equality is checked first and always, so a repeated sentence is
        // reported as a bank-hygiene signal rather than as retrieval tuning.
        let is_exact = !exact.is_empty() && kept_exact.contains(&exact);
        let is_near = near_duplicates
            && !tokens.is_empty()
            && kept_tokens
                .iter()
                .any(|seen| !seen.is_empty() && jaccard(seen, &tokens) >= cap);
        if is_exact {
            exact_dropped += 1;
        } else if is_near {
            near_dropped += 1;
        } else {
            kept_ids.insert(hit.id.clone());
            kept_tokens.push(tokens);
            kept_exact.push(exact);
        }
    }
    CapOutcome {
        kept_ids,
        exact_dropped,
        near_dropped,
    }
}

/// What one cap pass kept, and why the rest went.
struct CapOutcome {
    /// Ids of the surviving items, for the caller's drop bookkeeping.
    kept_ids: BTreeSet<String>,
    /// Items dropped because their text equalled a kept item's text.
    exact_dropped: usize,
    /// Items dropped by the widened near-duplicate tolerance.
    near_dropped: usize,
}

/// Lowercase, punctuation-free, whitespace-collapsed form used to decide
/// whether two items say the same thing (identical to the admission
/// exact-duplicate test, so write and read agree on what "duplicate" means).
///
/// Word order is preserved: "Page 1 Item 2" and "Page 2 Item 1" share a token
/// set but are different facts, so a set comparison must not decide this.
pub(crate) fn normalize_exact(text: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_space = false;
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        } else {
            pending_space = !out.is_empty();
        }
    }
    out
}

/// Lowercase alphanumeric word set (identical to the admission novelty proxy).
fn token_set(text: &str) -> BTreeSet<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Jaccard similarity of two token sets; both are non-empty at every call site.
fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let score = inter as f64 / union as f64;
    score
}
