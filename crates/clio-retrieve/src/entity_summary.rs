// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two entity-inclusion controls: the
//! request-level consistency rule between them, and the deterministic
//! top-level summary assembled from the entity names already on the page.
//!
//! ## Owns
//! - `EntitySummary`, the summary type (entity name -> number of returned hits)
//! - `summarize_entities`, the deterministic page summary
//! - `resolve_entity_controls`, the toggle/summary consistency rule
//!
//! ## Does not own
//! - The per-hit entity projection and its empty-coverage rules (`entities`)
//! - Applying the toggle to a hit (the page mapper `finalize` does)
//! - Request parsing (the bindings own that)
//!
//! ## Boundary
//!
//! Changes to the summary shape or the toggle/summary rule should normally
//! require changes here. Changes to where the toggle is applied, or to the
//! names themselves, should not require changes here.
//!
//! # The summary is a read-side roll-up, not new extraction
//!
//! Every name in the summary is already published on a returned hit's
//! `entities` array; nothing is extracted, linked, canonicalized, or inferred
//! here. The map is a `BTreeMap`, so the key order is name-ascending and two
//! calls over the same page serialize the same object.
//!
//! Entity names are content. The summary belongs on the authorized read
//! payload only: it must never reach `warnings`, the `explain` trace,
//! telemetry, or the health surfaces, exactly like the per-hit array.
//!
//! Keep this module focused on the two entity-inclusion controls.

use crate::types::ScoredHit;
use clio_types::{AmError, ErrorCode};
use std::collections::BTreeMap;

/// Every entity name present on the returned page, mapped to the number of
/// returned hits that carry it. Serializes as a JSON object keyed by name.
pub type EntitySummary = BTreeMap<String, usize>;

/// Count the entity names on one returned page.
///
/// Names are counted once per occurrence on a hit, so the counts sum to the
/// total number of names across the page. An empty page yields an empty map.
#[must_use]
pub fn summarize_entities(hits: &[ScoredHit]) -> EntitySummary {
    let mut summary = EntitySummary::new();
    for hit in hits {
        for name in &hit.entities {
            *summary.entry(name.clone()).or_insert(0) += 1;
        }
    }
    summary
}

/// Check the two entity controls against each other.
///
/// Asking for a summary while asking for no entity names is contradictory:
/// the summary is made of names, so honoring one would violate the other. The
/// combination is rejected rather than silently dropping the summary.
///
/// # Errors
/// `entity_summary` requested together with `include_entities` off.
pub fn resolve_entity_controls(
    include_entities: bool,
    entity_summary: bool,
) -> Result<(), AmError> {
    if entity_summary && !include_entities {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "retrieve entity_summary requires include_entities (an entity summary lists entity names)",
        ));
    }
    Ok(())
}
