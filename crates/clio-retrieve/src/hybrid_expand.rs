// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the optional bounded graph-expansion stage of
//! hybrid retrieval: seeding the expander, collecting its ids, and separating
//! the genuinely new ones.
//!
//! ## Owns
//! - `expand_candidates` and `newly_expanded`
//!
//! ## Does not own
//! - Graph traversal or edge storage (`clio_store`, `assoc`)
//! - Fusion, boosts, or the decay applied to expanded candidates (`hybrid_rank`)
//!
//! ## Boundary
//!
//! Changes to how expansion is seeded or bounded should normally require
//! changes here. Changes to fusion or the expander itself should not require
//! changes here.
//!
//! Keep this module focused on the bounded graph-expansion stage.

use crate::types::{GraphExpander, RetrieveRequest};
use clio_types::AmError;
use std::collections::BTreeSet;

/// Ids graph expansion added beyond the search legs' candidates.
pub(crate) fn newly_expanded(expanded: &BTreeSet<String>, candidates: &[String]) -> Vec<String> {
    expanded
        .iter()
        .filter(|id| !candidates.contains(id))
        .cloned()
        .collect()
}

/// Bounded graph expansion over the current candidate ids.
///
/// Returns the expanded id set and the ids no search leg produced. With
/// `expand_graph` off this is a no-op that returns two empty collections, so a
/// default call never touches the association graph.
pub(crate) fn expand_candidates(
    expander: &dyn GraphExpander,
    max_hops: usize,
    shortlist_m: usize,
    req: &RetrieveRequest,
    candidate_ids: &[String],
) -> Result<(BTreeSet<String>, Vec<String>), AmError> {
    if !req.expand_graph {
        return Ok((BTreeSet::new(), Vec::new()));
    }
    let seeds: Vec<String> = candidate_ids.iter().take(shortlist_m).cloned().collect();
    let reachable: BTreeSet<String> = expander
        .expand(&req.bank, &seeds, max_hops)?
        .into_iter()
        .collect();
    let graph_new = newly_expanded(&reachable, candidate_ids);
    Ok((reachable, graph_new))
}
