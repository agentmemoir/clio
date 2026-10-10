// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the candidate-generation legs of hybrid
//! retrieval: calling the dense and lexical search primitives (retaining each
//! hit's raw stage score) and reading the rank-only temporal window list.
//!
//! ## Owns
//! - `dense_candidates`, `lexical_candidates`, and `temporal_candidates`
//! - The dense leg's embed/search failure degradation (warning, empty list)
//!
//! ## Does not own
//! - Fusion, boosts, or rerank (`fusion`, `hybrid_rank`)
//! - Backend search SQL (`clio_store::IndexStore`) or embedding transport
//!   (`clio_index`), or window resolution (`temporal`)
//!
//! ## Boundary
//!
//! Changes to leg fetch/filter behavior or leg failure policy should normally
//! require changes here. Changes to fusion math should not require changes here.
//!
//! # Failure policy
//!
//! Dense coverage may lag the write path: an embed failure or dense-search
//! error degrades to lexical-only with a structured warning instead of failing
//! the query (index-lag contract). Lexical errors (empty bank/query) still fail
//! closed. The temporal leg fails closed on storage errors, like lexical.

use crate::fusion::StageHit;
use crate::temporal::{TemporalWindow, resolve_window};
use crate::types::RetrieveRequest;
use clio_index::{DenseSearchArgs, Embedder, LexicalSearchArgs, dense_search, lexical_search};
use clio_store::{IndexStore, Store};
use clio_types::AmError;
use std::sync::Arc;

/// Dense candidates with their raw cosine distance, best-first.
///
/// Returns an empty list with a structured warning when the embedder is absent,
/// the embed call fails, or the dense search fails.
pub(crate) fn dense_candidates(
    embedder: Option<&Arc<dyn Embedder>>,
    index: &dyn IndexStore,
    req: &RetrieveRequest,
    fetch: usize,
    warnings: &mut Vec<String>,
) -> Vec<StageHit> {
    let Some(embedder) = embedder else {
        return Vec::new();
    };
    let vectors = match embedder.embed_batch(std::slice::from_ref(&req.query)) {
        Ok(vectors) => vectors,
        Err(e) => {
            warnings.push(format!(
                "dense leg unavailable (embed failed): {}",
                e.message
            ));
            return Vec::new();
        }
    };
    let Some(vector) = vectors.first() else {
        return Vec::new();
    };
    match dense_search(
        index,
        &DenseSearchArgs {
            bank: &req.bank,
            query: vector,
            domains: req.domains.clone(),
            limit: fetch,
        },
    ) {
        Ok(hits) => hits
            .into_iter()
            .map(|h| StageHit {
                id: h.item_id,
                score: h.distance,
            })
            .collect(),
        Err(e) => {
            warnings.push(format!("dense leg unavailable: {}", e.message));
            Vec::new()
        }
    }
}

/// Lexical candidates with their raw backend score, best-first (fail-closed).
pub(crate) fn lexical_candidates(
    index: &dyn IndexStore,
    req: &RetrieveRequest,
    fetch: usize,
) -> Result<Vec<StageHit>, AmError> {
    let hits = lexical_search(
        index,
        &LexicalSearchArgs {
            bank: &req.bank,
            query: &req.query,
            domains: req.domains.clone(),
            limit: fetch,
        },
    )?;
    Ok(hits
        .into_iter()
        .map(|h| StageHit {
            id: h.item_id,
            score: h.score,
        })
        .collect())
}

/// Temporal-arm candidates inside `window`, best-first.
///
/// Rank-only: the arm returns ids with no score of any kind, so nothing from
/// it can enter a raw-score comparison. Ordering is deterministic (interval
/// start descending, then id ascending) and documented on
/// `Store::item_ids_in_window`. Bank scope, the axis' interval membership, and
/// the current-belief/discard eligibility all belong to that store query; the
/// shared candidate fetch then applies domains, archive, and `as_of`.
pub(crate) fn temporal_candidates(
    store: &dyn Store,
    req: &RetrieveRequest,
    window: &TemporalWindow,
    fetch: usize,
) -> Result<Vec<String>, AmError> {
    store.item_ids_in_window(&req.bank, req.time_axis, &window.start, &window.end, fetch)
}

/// Resolve the relative window (when the request carries one) and read the
/// rank-only temporal arm's candidate ids.
///
/// The temporal leg runs after the dense and lexical legs, so their work is
/// spent before the anchor is validated — but a malformed anchor still fails
/// closed as `invalid_argument` before the temporal arm contributes
/// candidates, so the request aborts. With no window (no anchor, or no
/// supported/ambiguous expression) the arm contributes nothing, so a
/// non-temporal query fuses exactly the two search legs.
pub(crate) fn temporal_leg(
    store: &dyn Store,
    req: &RetrieveRequest,
    fetch: usize,
) -> Result<(Option<TemporalWindow>, Vec<String>), AmError> {
    let window = match req.query_anchor.as_deref() {
        Some(anchor) => resolve_window(&req.query, anchor)?,
        None => None,
    };
    let ids = match &window {
        Some(window) => temporal_candidates(store, req, window, fetch)?,
        None => Vec::new(),
    };
    Ok((window, ids))
}
