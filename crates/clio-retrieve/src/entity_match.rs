// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deciding, deterministically, whether a query
//! text mentions one of a hit's entity names.
//!
//! ## Owns
//! - [`query_mentions_entity`] and [`query_matches_any`], the single match rule
//! - The documented normalization (Unicode lowercase mapping) and its limits
//!
//! ## Does not own
//! - The entity-name projection and its trim rule (`entities`)
//! - Attaching the flag to a hit, ordering, and page selection (`finalize`)
//! - Rendering the reason as text (`clio` renders the flag it is handed)
//!
//! ## Boundary
//!
//! Changes to the match rule or its normalization should normally require
//! changes here. Changes to ranking, hit assembly, or display should not
//! require changes here.
//!
//! # The rule
//!
//! A query mentions an entity name when the name, mapped to its Unicode
//! lowercase form, appears as a contiguous substring of the query mapped the
//! same way:
//!
//! ```text
//! match:    query "the Dark Mode toggle"   name "dark mode"
//! match:    query "restart the engine"     name "art"       (inside a token)
//! no match: query "dark themes"            name "dark mode"
//! ```
//!
//! Substring containment is the whole rule. It is not word matching, so a name
//! that happens to appear inside a longer token also matches; that consequence
//! is deliberate and documented rather than accidental. There is no fuzzy,
//! stemmed, phonetic, or approximate matching.
//!
//! # Normalization
//!
//! Both sides are mapped with [`str::to_lowercase`], the stable Unicode
//! lowercase mapping. That mapping is locale-independent and deterministic,
//! and its only contextual case (the Greek final sigma) is applied
//! language-independently by the standard library. Full Unicode case folding
//! is not stable in `core`, so it is not used and no dependency is added; the
//! practical limit is that a few case pairs do not match, for example "ß"
//! against "SS". Nothing else is normalized: no NFC/NFD normalization, no
//! whitespace collapsing, no punctuation stripping, and no length cap. Entity
//! names arrive already trimmed from the read-side projection; the query is
//! compared as given.
//!
//! Empty inputs never match: an empty query, an empty name (which the
//! projection never produces), or an empty name list all mean no match.
//!
//! # Display-only
//!
//! The result is derived at read time and is never verified against a source
//! span, so every caller must present it as derived and non-authoritative (the
//! `ScoredHit` doc comment and the `clio recall` text view carry that label).
//! It is computed after fusion and no ranking code reads it: ordering, scores,
//! filters, and budgets are identical whether or not a hit matches.
//!
//! # Determinism
//!
//! Both functions are pure: no clock, no environment, no randomness. Identical
//! input yields an identical result.

/// True when `query` mentions the entity name `entity` under the rule above.
///
/// The name is expected to be the trimmed value the read-side projection
/// exposes; this function does not trim, normalize Unicode, or rewrite either
/// side beyond the lowercase mapping.
#[must_use]
pub(crate) fn query_mentions_entity(query: &str, entity: &str) -> bool {
    if query.is_empty() || entity.is_empty() {
        return false;
    }
    query.to_lowercase().contains(&entity.to_lowercase())
}

/// True when `query` mentions at least one name in `entities`.
///
/// An empty list never matches. The order of the list does not matter: the
/// answer is the same for any permutation.
#[must_use]
pub(crate) fn query_matches_any(query: &str, entities: &[String]) -> bool {
    entities
        .iter()
        .any(|entity| query_mentions_entity(query, entity))
}

#[cfg(test)]
#[path = "entity_match_tests.rs"]
mod entity_match_tests;
