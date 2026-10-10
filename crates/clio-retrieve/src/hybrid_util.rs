// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure helper functions shared by the hybrid
//! orchestrator and the finalize/compose stages: leg-weight normalization,
//! candidate filtering, rank maps, deterministic ordering, and doc text.
//!
//! ## Owns
//! - `normalize_leg_weights`, `kept_stage`, `rank_map`, `dedupe_ordered`,
//!   `doc_text`, `millis`, `sort_fused`, `importance_of`
//!
//! ## Does not own
//! - Retrieval orchestration (`hybrid`), fusion math (`fusion`), packing (`compose`)
//!
//! ## Boundary
//!
//! Changes to these shared helpers should normally require changes here.
//! Changes to retrieval control flow should not require changes here.

use crate::fusion::{FusedHit, StageHit};
use clio_config::ranking::RetrievalWeights;
use clio_types::MemoryItem;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// Normalize dense/lexical leg weights to sum 1 (equal split when degenerate).
pub(crate) fn normalize_leg_weights(w: &RetrievalWeights) -> (f64, f64) {
    let sum = w.dense + w.lexical;
    if sum <= 0.0 || !sum.is_finite() {
        (0.5, 0.5)
    } else {
        (w.dense / sum, w.lexical / sum)
    }
}

/// Keep stage hits present in the fetched item map, preserving order.
pub(crate) fn kept_stage(hits: &[StageHit], items: &BTreeMap<String, MemoryItem>) -> Vec<StageHit> {
    hits.iter()
        .filter(|hit| items.contains_key(&hit.id))
        .cloned()
        .collect()
}

/// 1-based rank map for a stage-hit list.
pub(crate) fn rank_map(hits: &[StageHit]) -> BTreeMap<String, usize> {
    hits.iter()
        .enumerate()
        .map(|(i, hit)| (hit.id.clone(), i + 1))
        .collect()
}

/// De-duplicate ids preserving first-seen order.
pub(crate) fn dedupe_ordered<'a>(ids: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for id in ids {
        if seen.insert(id.clone()) {
            out.push(id.clone());
        }
    }
    out
}

/// Lexical source text used for token accounting and rerank documents.
pub(crate) fn doc_text(item: Option<&MemoryItem>) -> String {
    item.and_then(clio_index::lexical_text).unwrap_or_default()
}

/// Elapsed milliseconds, saturating rather than wrapping.
pub(crate) fn millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Deterministic fused ordering: score desc, importance desc, then id.
pub(crate) fn sort_fused(fused: &mut [FusedHit], items: &BTreeMap<String, MemoryItem>) {
    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                importance_of(b, items)
                    .partial_cmp(&importance_of(a, items))
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn importance_of(hit: &FusedHit, items: &BTreeMap<String, MemoryItem>) -> f64 {
    items
        .get(&hit.id)
        .and_then(|i| i.admission_score)
        .unwrap_or(0.0)
}
