// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for projecting an item's snapshot entity name
//! into the per-hit `entities[]` array of a retrieval result.
//!
//! ## Owns
//! - [`snapshot_entities`], the single read-side source of hit entity names
//! - The documented empty-coverage cases and the trim-only canonicalization rule
//!
//! ## Does not own
//! - Snapshot verification at write time (`clio-write` owns span verification)
//! - Hit assembly, ordering, or ranking (`finalize` owns the result mapping)
//! - Entity linking, deduplication across items, or NER of any kind
//!
//! # Source and authority
//!
//! The only source is the item's own `snapshot.entity` string. On the write
//! paths that span-verify (the default `store`/`batch` admission path) that
//! value had to appear as a contiguous NFC source substring, so there it is
//! the already-verified value a read may expose. Two write paths store a
//! caller-supplied snapshot without re-running that check: a bundle import
//! stages the restored item as supplied, and an operator correction
//! (`correct`/`update`) stores the caller's snapshot whole. An `entity`
//! written through those paths is republished here as stored and was not
//! span-verified; this adds no new exposure class, because a caller authorized
//! to read the item can read its whole snapshot either way. Nothing is
//! inferred from the gist, from subject/predicate/object triples, or from
//! association edges: a retrieval-side link would be inferred rather than
//! verified, so it is not authoritative and is not exposed. The value is
//! descriptive hit metadata, never a truth claim.
//!
//! # Canonicalization
//!
//! Trim only, using the same `str::trim` the write-side span matcher applied. The
//! stored string is returned unchanged otherwise: no case folding, no Unicode
//! normalization, no truncation, no length cap, and no deduplication. A name that
//! is empty after trimming is dropped, matching the write-side rule that rejects
//! an empty entity value.
//!
//! One consequence is worth stating plainly. The write-side matcher compared the
//! NFC form of the stored string against the source, so for a value stored in a
//! decomposed Unicode form the span that was verified is the NFC text. This read
//! does not re-normalize, so it exposes the stored text with its whitespace
//! trimmed. Re-normalizing here would need a Unicode-normalization dependency in
//! this crate, which this change deliberately does not add; a test pins the
//! trim-only behavior so a later change to it is a deliberate decision.
//!
//! # Empty coverage
//!
//! `entities` is `[]` — an expected, documented result, not a partial or broken
//! one — for every hit kind that carries no `entity` string to expose (see
//! "Source and authority": an unverified value written through the import or
//! correction paths is still exposed when present):
//!
//! | Hit kind | Why it is empty |
//! |----------|-----------------|
//! | Triple carrier | Stores `{subject, predicate, object}`; no `entity` field. |
//! | Hub-distilled hub | The distilled snapshot has no `entity` field. |
//! | Belief object | Belongs to the belief domain, which has no snapshot. |
//! | Snapshot-less item | `snapshot` is `None`. |
//! | Non-default field list | The write path asked for other extractive fields. |
//! | Erased / unreadable item | The content no longer decrypts, so the item cannot be a hit. |
//!
//! # Boundary
//!
//! Entity names are content-derived. They belong on the authorized read payload
//! and nowhere else: this projection is never added to the `warnings` list, the
//! explanation trace, telemetry events, or the health surfaces. Changes to the
//! source, the trim rule, or the empty-coverage set should normally require
//! changes here.

use clio_types::MemoryItem;

/// The snapshot field that carries the item's entity name.
pub(crate) const SNAPSHOT_ENTITY_FIELD: &str = "entity";

/// Entity names for one item, sourced only from its snapshot value.
///
/// On the span-verifying write paths (`store`/`batch`) that value was verified
/// against the source text at write time; a bundle-imported or operator-
/// corrected item carries the snapshot as supplied, so its name was not
/// span-verified.
///
/// Returns a single-element vector when the item's snapshot has a non-empty
/// string `entity`, and an empty vector for every other shape: no snapshot, a
/// non-object snapshot, a missing `entity`, a non-string `entity`, or an `entity`
/// that is empty after trimming. See the module docs for the empty-coverage table
/// and the trim-only canonicalization rule.
#[must_use]
pub(crate) fn snapshot_entities(item: &MemoryItem) -> Vec<String> {
    let Some(snapshot) = item.snapshot.as_ref() else {
        return Vec::new();
    };
    let Some(name) = snapshot.get(SNAPSHOT_ENTITY_FIELD).and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    vec![trimmed.to_owned()]
}

#[cfg(test)]
#[path = "entities_tests.rs"]
mod entities_tests;

#[cfg(test)]
#[path = "entities_entity_controls_tests.rs"]
mod entities_entity_controls_tests;
