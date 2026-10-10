// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for fetching one candidate id set into the
//! retrieved item map under the request's visibility and domain rules.
//!
//! ## Owns
//! - The candidate-by-candidate store read loop for every retrieval arm
//! - The unreadable-content skip policy and the archive/probe handling
//! - The domain, `as_of`/`time_axis`, archive, and provenance-floor filters
//!   applied at fetch time
//!
//! ## Does not own
//! - Which ids become candidates (the arms: `hybrid_legs`, `hybrid`)
//! - Fusion, boosts, rerank, or finalize (`fusion`, `hybrid_rank`, `finalize`)
//!
//! ## Boundary
//!
//! Changes to candidate fetch/filter behavior should normally require changes
//! here. Changes to arm candidate generation should not require changes here.
//!
//! # Failure policy
//!
//! A candidate whose content is unreadable (missing or crypto-shredded DEK) is
//! skipped with a warning; one unreadable candidate never aborts the query. A
//! broken backend still errors, so a query never answers from a store that is
//! actually broken.

use crate::filter::{item_in_domains, item_visible_at};
use crate::floors::provenance_floor_keeps;
use crate::types::RetrieveRequest;
use clio_store::Store;
use clio_types::{AmError, MemoryItem, SearchDomain};
use std::collections::BTreeMap;

/// Fetch the candidate items for one query, preserving the arms' order.
///
/// Each id is resolved by bank; a missing item, an item outside the requested
/// domains, an item not visible at `as_of` on `time_axis`, an archived item,
/// and an item below the request's provenance floor are all dropped from the
/// map. The arms then keep only the stage hits whose ids survived, so all
/// filtering happens in one place: one prune removes the item from the returned
/// page, the dedup scope, graph inclusion, and the reinforced set alike.
pub(crate) fn fetch_items(
    store: &dyn Store,
    bank: &str,
    ids: &[String],
    domains: &[SearchDomain],
    req: &RetrieveRequest,
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<String, MemoryItem>, AmError> {
    let mut items = BTreeMap::new();
    for id in ids {
        let archived = store.item_is_archived(bank, id)?;
        let item = match store.get_memory_item(bank, id) {
            Ok(item) => item,
            // Unreadable content skips the candidate instead of aborting the
            // whole query. Same predicate as the admission neighbor scan, so
            // the two readers cannot drift apart: a damaged envelope, an
            // undecodable nonce or ciphertext, a missing content column, a
            // content version this build cannot read, a missing or shredded
            // DEK. A broken backend still errors, so a query never answers
            // from a store that is actually broken. `get` on the id still
            // reports the error.
            Err(e) if clio_store::is_unreadable_content(&e) => {
                warnings.push(format!("skipped unreadable item `{id}`: {}", e.message));
                continue;
            }
            Err(e) => return Err(e),
        };
        if let Some(item) = item {
            let visible = match &req.as_of {
                None => true,
                Some(as_of) => item_visible_at(&item, req.time_axis, as_of),
            };
            if visible
                && !archived
                && item_in_domains(&item, domains)
                && provenance_floor_keeps(&item, req.provenance_floor)
            {
                items.insert(id.clone(), item);
            }
        }
    }
    Ok(items)
}
