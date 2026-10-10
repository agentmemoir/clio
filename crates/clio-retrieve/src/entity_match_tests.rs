// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the entity-overlap match rule and its projection: the
//! case/normalization rule at the unit level, the derived flag on a finalized
//! hit through a real retrieve, the empty and no-mention cases, determinism,
//! and the ranking invariance of the flag.
//!
//! ## Owns
//! - `query_mentions_entity` / `query_matches_any` unit assertions
//! - End-to-end `ScoredHit.entity_match` assertions and payload shape
//! - The dense-leg variant of the flag (embedder attached)
//!
//! ## Does not own
//! - The entity-name projection (`entities_tests`, `hit_entities_tests`)
//! - Binding parity and the diagnostic boundary (`clio-mcp`, `clio`)
//! - The rendered text line (tested in `clio`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the entity-overlap reason contract.

use crate::entity_match::{query_matches_any, query_mentions_entity};
use crate::fixtures_tests as fx;
use crate::hybrid::HybridRetriever;
use crate::types::{RetrieveOutcome, RetrieveRequest, ScoredHit};
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "match-bank";

/// Frozen clock so recency is identical across the two retrievals in a test.
const FROZEN_NOW: &str = "2026-06-01T00:00:00Z";

/// A semantic item in the fixture bank whose snapshot is the given value.
fn item_with_snapshot(id: &str, gist: &str, snapshot: Value) -> clio_types::MemoryItem {
    let mut item = fx::semantic_item(id, BANK, gist);
    item.snapshot = Some(snapshot);
    item
}

/// Retrieve one lexical hit for `query` and return the matching hit.
fn hit_for(entity: &str, query: &str) -> ScoredHit {
    let store = fx::store();
    let item = item_with_snapshot(
        "itm-1",
        "a note that mentions nothing by name",
        json!({"entity": entity, "amount": 1, "date": "2024-01-01"}),
    );
    fx::put_lexical(&store, &item, &format!("{entity} retrieval note"));
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&RetrieveRequest::new(BANK, query))
        .expect("retrieve");
    out.hits
        .into_iter()
        .find(|h| h.hit.item_id == "itm-1")
        .unwrap_or_else(|| panic!("{entity} hit present"))
}

/// The same-case name reaches the derived flag on the hit.
#[test]
fn exact_name_in_query_matches() {
    assert!(query_mentions_entity(
        "the dark mode toggle is on",
        "dark mode"
    ));
    let hit = hit_for("dark mode", "the dark mode toggle is on");
    assert!(hit.entity_match, "the query mentions the name: {hit:?}");
    assert_eq!(hit.entities, vec!["dark mode".to_owned()]);
}

/// Case differences follow the documented lowercase rule in both
/// directions.
#[test]
fn case_difference_follows_the_lowercase_rule() {
    assert!(query_mentions_entity("THE DARK MODE TOGGLE", "dark mode"));
    assert!(query_mentions_entity("dark mode", "DARK MODE"));
    assert!(hit_for("Dark Mode", "the DARK MODE toggle").entity_match);
}

/// Substring containment is the rule, so a name inside a longer token matches.
/// This consequence is documented in the module header, not accidental.
#[test]
fn a_name_inside_a_longer_token_matches() {
    assert!(query_mentions_entity("restart the engine", "art"));
    assert!(!query_mentions_entity("a longer phrase", "art"));
}

/// Query whitespace is compared as given: surrounding whitespace is harmless
/// for containment, while internal whitespace differences are not collapsed.
#[test]
fn whitespace_and_punctuation_are_compared_as_given() {
    assert!(query_mentions_entity("   dark mode   ", "dark mode"));
    assert!(!query_mentions_entity("dark  mode", "dark mode"));
    assert!(!query_mentions_entity("dark-mode", "dark mode"));
}

/// An empty name list never matches, and one name is enough.
#[test]
fn any_one_of_several_names_matches() {
    let names = vec!["alpha".to_owned(), "beta".to_owned()];
    assert!(query_matches_any("the beta readout", &names));
    assert!(!query_matches_any("the gamma readout", &names));
    // Order does not matter.
    let reversed: Vec<String> = names.iter().rev().cloned().collect();
    assert!(query_matches_any("the beta readout", &reversed));
}

/// Empty inputs never match: no query, no name, or no list.
#[test]
fn empty_inputs_never_match() {
    assert!(!query_mentions_entity("", "dark mode"));
    assert!(!query_mentions_entity("dark mode", ""));
    assert!(!query_matches_any("dark mode", &[]));
    let empty_name = vec![String::new()];
    assert!(!query_matches_any("dark mode", &empty_name));
}

/// The lowercase mapping is Unicode-aware (so a Turkish dotted capital matches
/// its folded form) but is not full case folding, so "ß" does not match "SS".
#[test]
fn the_lowercase_mapping_is_unicode_aware_and_not_full_case_folding() {
    // `to_lowercase('İ')` is `i` plus U+0307 COMBINING DOT ABOVE.
    let folded = "i\u{307}stanbul";
    assert!(query_mentions_entity("İSTANBUL office", folded));
    assert!(!query_mentions_entity("STRASSE 5", "straße 5"));
}

/// The rule is a pure function of its inputs.
#[test]
fn the_rule_is_deterministic_for_identical_input() {
    let cases = [
        ("the dark mode toggle", "dark mode"),
        ("THE DARK MODE TOGGLE", "dark mode"),
        ("restart the engine", "art"),
        ("a longer phrase", "art"),
        ("", "dark mode"),
        ("dark mode", ""),
    ];
    for (query, entity) in cases {
        let first = query_mentions_entity(query, entity);
        let second = query_mentions_entity(query, entity);
        assert_eq!(first, second, "`{query}` / `{entity}` is deterministic");
    }
}

/// The flag is absent from the payload when no name is mentioned, so a
/// non-matching hit serializes exactly the pre-existing shape.
#[test]
fn a_query_without_the_name_leaves_the_flag_off() {
    let hit = hit_for("dark mode", "retrieval note");
    assert!(!hit.entity_match, "no mention: {hit:?}");
    let value = serde_json::to_value(&hit).expect("serialize hit");
    assert!(
        value.get("entity_match").is_none(),
        "the false flag is omitted: {value}"
    );
}

/// The dense leg exercises the same projection: a hit ranked by the dense arm
/// (embedder attached, `dense_rank` set, semantic score present) carries the
/// derived flag when the query mentions its entity name, so leg-independence
/// is executed rather than asserted.
#[test]
fn dense_populated_hit_carries_the_derived_flag() {
    let store = fx::store();
    let item = item_with_snapshot(
        "itm-dense",
        "note about the dense leg entity",
        json!({"entity": "dense leg entity", "amount": 1, "date": "2024-01-01"}),
    );
    fx::put_vector(&store, &item, fx::vector(7));
    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: fx::vector(7),
    }));
    let out = retriever
        .retrieve(&RetrieveRequest::new(BANK, "dense leg entity"))
        .expect("retrieve");
    let hit = out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "itm-dense")
        .unwrap_or_else(|| panic!("dense hit present: {:?}", out.hits));
    assert!(hit.entity_match, "the mentioned name is marked: {hit:?}");
    assert_eq!(hit.entities, vec!["dense leg entity".to_owned()], "{hit:?}");
    assert_eq!(
        hit.dense_rank,
        Some(1),
        "the hit came from the dense leg: {hit:?}"
    );
    assert!(
        hit.scores.semantic.is_some(),
        "the dense arm scored the hit: {hit:?}"
    );
}

/// The mention must be against the hit's own names, so a mention with no
/// entity to match never sets the flag.
#[test]
fn a_hit_with_no_entity_never_matches() {
    let store = fx::store();
    let plain = item_with_snapshot("itm-plain", "dark mode note", json!({"amount": 1}));
    fx::put_lexical(&store, &plain, "dark mode note");
    let triple = item_with_snapshot(
        "itm-triple",
        "dark mode note",
        json!({"subject": "rust", "predicate": "used_by", "object": "agent"}),
    );
    fx::put_lexical(&store, &triple, "dark mode note");

    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&RetrieveRequest::new(BANK, "dark mode"))
        .expect("retrieve");
    assert_eq!(out.hits.len(), 2, "{:?}", out.hits);
    for hit in &out.hits {
        assert!(
            hit.entities.is_empty(),
            "{} exposes no name: {hit:?}",
            hit.hit.item_id
        );
        assert!(
            !hit.entity_match,
            "{} must not match without a name: {hit:?}",
            hit.hit.item_id
        );
    }
}

/// The flag never changes ordering or fused scores. The same
/// query and frozen clock run over two stores that differ only in whether the
/// snapshots carry an `entity`; the matching store gets the flag and the plain
/// store does not, while both keep identical order and scores.
#[test]
fn matching_never_changes_order_or_scores() {
    let order = |out: &RetrieveOutcome| -> Vec<String> {
        out.hits.iter().map(|h| h.hit.item_id.clone()).collect()
    };
    let scores = |out: &RetrieveOutcome| -> Vec<String> {
        out.hits
            .iter()
            .map(|h| format!("{:.17}", h.score))
            .collect()
    };
    let mut request = RetrieveRequest::new(BANK, "itm-r1 match probe");
    request.now = Some(FROZEN_NOW.to_owned());

    let named_out = ranking_store(true).retrieve(&request).expect("named");
    let plain_out = ranking_store(false).retrieve(&request).expect("plain");
    assert!(!order(&named_out).is_empty(), "hits present: {named_out:?}");
    assert_eq!(order(&named_out), order(&plain_out), "hit order");
    assert_eq!(scores(&named_out), scores(&plain_out), "fused scores");

    let named_r1 = named_out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "itm-r1")
        .expect("r1 named hit");
    assert!(named_r1.entity_match, "{named_r1:?}");
    assert!(
        named_out
            .hits
            .iter()
            .filter(|h| h.hit.item_id != "itm-r1")
            .all(|h| !h.entity_match),
        "only the mentioned hit is marked: {named_out:?}"
    );
    assert!(
        plain_out.hits.iter().all(|h| !h.entity_match),
        "no names, no flag: {plain_out:?}"
    );
}

/// One store holding the same two items, differing only in whether their
/// snapshots carry an `entity` value.
fn ranking_store(with_entity: bool) -> HybridRetriever {
    let store = fx::store();
    for id in ["itm-r1", "itm-r2"] {
        // The text carries the id so the two items are distinct facts. A shared
        // text would make them exact duplicates, and read-time collapse would
        // drop one before the order comparison below ever ran.
        let item = item_with_snapshot(
            id,
            &format!("entity match ranking note {id}"),
            if with_entity {
                json!({"entity": format!("{id} match probe")})
            } else {
                json!({"amount": 1})
            },
        );
        fx::put_lexical(
            &store,
            &item,
            &format!("entity match ranking note {id} padding"),
        );
    }
    fx::retriever(&store)
}

/// The same store, query, and frozen clock produce the same hit
/// order, scores, and flags on a second run.
#[test]
fn identical_input_yields_identical_reason_and_order() {
    let store = fx::store();
    for id in ["itm-d1", "itm-d2"] {
        let item = item_with_snapshot(
            id,
            "determinism note",
            json!({"entity": format!("{id} probe")}),
        );
        fx::put_lexical(&store, &item, "determinism note");
    }
    let retriever = fx::retriever(&store);
    let mut request = RetrieveRequest::new(BANK, "itm-d1 probe determinism");
    request.now = Some(FROZEN_NOW.to_owned());

    let first = retriever.retrieve(&request).expect("first");
    let second = retriever.retrieve(&request).expect("second");
    assert_eq!(first.hits.len(), 2, "{first:?}");
    for (a, b) in first.hits.iter().zip(second.hits.iter()) {
        assert_eq!(a.hit.item_id, b.hit.item_id, "same order");
        assert_eq!(a.score.to_bits(), b.score.to_bits(), "same score");
        assert_eq!(a.entity_match, b.entity_match, "same reason");
    }
    assert_eq!(
        first.hits.iter().filter(|h| h.entity_match).count(),
        1,
        "exactly one mention: {first:?}"
    );
}
