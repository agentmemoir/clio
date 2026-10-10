// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `entities[]` array on a finalized retrieval hit: that a
//! verified snapshot entity reaches the hit through a real retrieve, that every
//! empty-coverage shape survives the whole pipeline as `[]`, and that the array
//! never changes ranking, the compose pack, or a diagnostic surface.
//!
//! ## Owns
//! - End-to-end `entities` assertions on `RetrieveOutcome.hits`
//! - The dense-leg variant of the projection (embedder attached)
//! - Frozen-contract and compose-pack invariance for the new field
//!
//! ## Does not own
//! - The projection unit contract (`entities_tests`)
//! - Binding payload parity and the PII boundary (`clio-mcp`, `clio`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the finalized-hit entity contract.

use crate::compose::{ComposeRequest, Composer};
use crate::fixtures_tests as fx;
use crate::types::{RetrieveRequest, ScoredHit};
use clio_types::{MemoryItem, SourceType};
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "ent-bank";

/// A semantic item in the fixture bank whose snapshot is the given value.
fn item_with_snapshot(id: &str, gist: &str, snapshot: Value) -> MemoryItem {
    let mut item = fx::semantic_item(id, BANK, gist);
    item.snapshot = Some(snapshot);
    item
}

/// The default-field write shape: `entity` plus the other extractive fields.
fn entity_item(id: &str, entity: &str) -> MemoryItem {
    item_with_snapshot(
        id,
        &format!("note about {entity}"),
        json!({"entity": entity, "amount": 1, "date": "2024-01-01"}),
    )
}

/// Retrieve one lexical hit for `query` and return the matching hit.
fn hit_for(entity: &str, query: &str) -> ScoredHit {
    let store = fx::store();
    let item = entity_item("itm-1", entity);
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

/// The verified snapshot entity reaches the finalized hit.
#[test]
fn verified_snapshot_entity_reaches_the_finalized_hit() {
    let hit = hit_for("dark mode", "dark");
    assert_eq!(hit.entities, vec!["dark mode".to_owned()]);
    assert!(
        hit.snapshot_ref.as_deref() == Some("itm-1"),
        "the hit carries the snapshot ref: {hit:?}"
    );
}

/// a stored value with surrounding whitespace is trimmed on the hit.
#[test]
fn surrounding_whitespace_is_trimmed_on_the_finalized_hit() {
    let hit = hit_for("  Ada Lovelace  ", "Ada");
    assert_eq!(hit.entities, vec!["Ada Lovelace".to_owned()]);
}

/// the empty-coverage shapes stay empty through a real
/// retrieve, so `[]` is a documented result rather than a broken one.
#[test]
fn empty_coverage_shapes_survive_the_pipeline_as_empty_arrays() {
    let store = fx::store();
    let shapes: Vec<(&str, Option<Value>)> = vec![
        (
            "itm-triple",
            Some(json!({"subject": "rust", "predicate": "used_by", "object": "agent"})),
        ),
        (
            "itm-hub",
            Some(json!({"hub_id": "itm-hub", "neighbor_ids": [], "neighbor_count": 0})),
        ),
        ("itm-metric", Some(json!({"metric": "p95"}))),
        ("itm-num", Some(json!({"entity": 42}))),
        ("itm-blank", Some(json!({"entity": "   "}))),
        ("itm-none", None),
    ];
    for (id, snapshot) in &shapes {
        // The gist carries the id so every item is a distinct fact. A shared
        // gist would make them exact duplicates, and read-time collapse would
        // drop all but one before the count below was reached.
        let mut item = fx::semantic_item(id, BANK, &format!("coverage note {id}"));
        item.snapshot = snapshot.clone();
        fx::put_lexical(&store, &item, &format!("coverage note {id}"));
    }
    let mut belief = fx::belief_item("itm-belief", BANK, "belief note");
    belief.source_type = Some(SourceType::AgentInferred);
    belief.snapshot = None;
    fx::put_lexical(&store, &belief, "coverage note");

    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&RetrieveRequest::new(BANK, "coverage"))
        .expect("retrieve");
    assert_eq!(out.hits.len(), shapes.len() + 1, "{:?}", out.hits);
    for hit in &out.hits {
        assert_eq!(
            hit.entities,
            Vec::<String>::new(),
            "{} must expose no entity name: {hit:?}",
            hit.hit.item_id
        );
    }
}

/// The dense leg exercises the same projection: a hit ranked by the dense arm
/// (embedder attached, `dense_rank` set, semantic score present) carries the
/// same snapshot entity, so leg-independence is executed rather than asserted.
#[test]
fn dense_populated_hit_carries_the_same_entities() {
    let store = fx::store();
    let item = entity_item("itm-dense", "dense leg entity");
    fx::put_vector(&store, &item, fx::vector(7));
    let mut retriever = fx::retriever(&store);
    retriever.set_embedder(Arc::new(fx::FixedEmbedder {
        vector: fx::vector(7),
    }));
    let out = retriever
        .retrieve(&RetrieveRequest::new(BANK, "dense"))
        .expect("retrieve");
    let hit = out
        .hits
        .iter()
        .find(|h| h.hit.item_id == "itm-dense")
        .unwrap_or_else(|| panic!("dense hit present: {:?}", out.hits));
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

/// The array is always present in the JSON payload, empty included, so a
/// consumer can read one key without branching on absence.
#[test]
fn every_hit_serializes_the_entities_key_even_when_empty() {
    let store = fx::store();
    let named = entity_item("itm-named", "named entity");
    fx::put_lexical(&store, &named, "named entity note");
    let plain = fx::semantic_item("itm-plain", BANK, "plain note");
    fx::put_lexical(&store, &plain, "plain note note");

    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&RetrieveRequest::new(BANK, "note"))
        .expect("retrieve");
    let value = serde_json::to_value(&out).expect("serialize outcome");
    for hit in value["hits"].as_array().expect("hits array") {
        let entities = hit
            .get("entities")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("entities key present: {hit}"));
        assert!(
            entities.iter().all(Value::is_string),
            "only strings in entities: {hit}"
        );
    }
    let named = value["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .find(|h| h["item_id"] == json!("itm-named"))
        .expect("named hit");
    assert_eq!(named["entities"], json!(["named entity"]), "{named}");
}

/// The frozen fact/belief contract is untouched. `entities` is new
/// hit metadata on `ScoredHit`; every `RetrieveHit` field keeps its old value.
#[test]
fn the_frozen_retrieve_hit_contract_is_unchanged() {
    let hit = hit_for("contract entity", "contract");
    let value = serde_json::to_value(&hit).expect("serialize hit");
    for (key, expected) in [
        ("item_id", json!("itm-1")),
        ("bank_id", json!(BANK)),
        ("kind", json!("semantic")),
        ("category", json!("schema")),
        ("epistemic_kind", json!("fact")),
        ("requires_confidence_check", json!(false)),
    ] {
        assert_eq!(value[key], expected, "frozen field `{key}`: {value}");
    }
    assert!(
        value.get("source_type").is_none() && value.get("confidence").is_none(),
        "a fact still carries no belief provenance: {value}"
    );
    assert_eq!(value["entities"], json!(["contract entity"]), "{value}");
}

/// The new field carries no ranking information: the same store, query, and
/// frozen clock return the same hit order and fused scores whether or not the
/// items carry a snapshot entity, so entity names cannot leak into fusion.
#[test]
fn entities_do_not_affect_hit_order_or_scores() {
    let order = |out: &crate::types::RetrieveOutcome| -> Vec<String> {
        out.hits.iter().map(|h| h.hit.item_id.clone()).collect()
    };
    let scores = |out: &crate::types::RetrieveOutcome| -> Vec<String> {
        out.hits
            .iter()
            .map(|h| format!("{:.17}", h.score))
            .collect()
    };
    let mut request = RetrieveRequest::new(BANK, "ranking comparison");
    request.now = Some(FROZEN_NOW.to_owned());

    let named_out = ranking_store(true)
        .retrieve(&request)
        .expect("retrieve named");
    let plain_out = ranking_store(false)
        .retrieve(&request)
        .expect("retrieve plain");
    assert!(!order(&named_out).is_empty(), "hits present: {named_out:?}");
    assert_eq!(order(&named_out), order(&plain_out), "hit order");
    assert_eq!(scores(&named_out), scores(&plain_out), "fused scores");
    assert!(named_out.hits.iter().all(|h| !h.entities.is_empty()));
    assert!(plain_out.hits.iter().all(|h| h.entities.is_empty()));
}

/// Frozen clock so the recency term is identical in both retrievals.
const FROZEN_NOW: &str = "2026-06-01T00:00:00Z";

/// One store holding the same two items, differing only in whether their
/// snapshots carry an `entity` value.
fn ranking_store(with_entity: bool) -> crate::hybrid::HybridRetriever {
    let store = fx::store();
    for id in ["itm-r1", "itm-r2"] {
        // The text carries the id so the two items are distinct facts. A shared
        // text would make them exact duplicates, and read-time collapse would
        // drop one before the order comparison below ever ran.
        let mut item = fx::semantic_item(id, BANK, &format!("ranking comparison note {id}"));
        item.snapshot = Some(if with_entity {
            json!({"entity": format!("{id} entity")})
        } else {
            json!({"amount": 1})
        });
        fx::put_lexical(
            &store,
            &item,
            &format!("ranking comparison note {id} padding"),
        );
    }
    fx::retriever(&store)
}

/// The compose pack stays gist-only: an entity name never enters the injected
/// text, so the budget contract is unchanged by this field.
#[test]
fn entity_names_never_enter_the_compose_pack() {
    let store = fx::store();
    // The gist deliberately does not mention the entity, so a name in the pack
    // could only have come from `entities[]`.
    let item = item_with_snapshot(
        "itm-pack",
        "a pack note that mentions nothing by name",
        json!({"entity": "PackEntityName", "amount": 1}),
    );
    fx::put_lexical(&store, &item, "PackEntityName pack note");
    let retriever = fx::retriever(&store);
    let composer = Composer::new(std::sync::Arc::new(retriever));
    let mut request = ComposeRequest::new(BANK);
    request.query = Some("PackEntityName".to_owned());
    request.now = Some(FROZEN_NOW.to_owned());
    let pack = composer.compose(&request, None).expect("compose");
    assert!(pack.retrieval_ran, "retrieval ran: {pack:?}");
    let text = pack
        .sections
        .iter()
        .map(|s| s.text.as_str())
        .collect::<String>();
    assert!(
        text.contains("a pack note that mentions nothing by name"),
        "the gist is still packed: {text}"
    );
    assert!(
        !text.contains("PackEntityName"),
        "no entity name in the pack text: {text}"
    );
}
