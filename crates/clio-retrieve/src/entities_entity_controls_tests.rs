// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the entity-inclusion controls over the real
//! retrieval path: the toggle suppressing per-hit names, the deterministic
//! top-level summary, the rule that rejects a summary without names, and the
//! content boundary the summary must keep.
//!
//! ## Owns
//! - `include_entities` on/off assertions over the returned page
//! - `entity_summary` counting, key order, and absence unless requested
//! - The rejection of `entity_summary` with `include_entities: false`, through
//!   both the retriever and the in-process JSON surface
//! - The boundary sweep proving a name reaches only the hit array and the
//!   summary, never the rest of the payload
//!
//! ## Does not own
//! - The per-hit projection rules (`entities_tests`, `hit_entities_tests`)
//! - The trace and health sweeps at the MCP binding (`clio-mcp` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the entity-inclusion controls.

use crate::fixtures_tests as fx;
use crate::types::RetrieveRequest;
use clio_types::ErrorCode;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// An entity name no other field carries, so a boundary sweep that finds it can
/// only have found it in a hit's `entities` array or in the summary.
const NAME: &str = "EntToggleName42";
const OTHER: &str = "EntToggleOther17";

/// Request with a frozen clock so recency boosts stay deterministic.
fn request(bank: &str, query: &str) -> RetrieveRequest {
    let mut req = RetrieveRequest::new(bank, query);
    req.now = Some("2026-01-02T00:00:00Z".into());
    req
}

/// A semantic item whose snapshot carries `entity`.
fn named_item(id: &str, bank: &str, entity: &str, text: &str) -> clio_types::MemoryItem {
    let mut entry = fx::item(id, bank, text);
    entry.snapshot = Some(json!({"entity": entity, "amount": 1}));
    entry
}

/// Seed two items sharing one name and one item with another name.
///
/// Each item's text carries its own id so the three are distinct facts. Sharing
/// one text would make them exact duplicates, and read-time collapse would drop
/// two of them before this suite ever saw a page.
fn seed(store: &std::sync::Arc<clio_store::SqliteStore>, bank: &str) {
    for (id, entity) in [("A", NAME), ("B", NAME), ("C", OTHER)] {
        let entry = named_item(id, bank, entity, &format!("alpha memory token {id}"));
        fx::put_lexical(store, &entry, &format!("alpha memory token {id}"));
    }
}

fn names_of(hit: &crate::types::ScoredHit) -> Vec<String> {
    hit.entities.clone()
}

fn summary_of(out: &crate::types::RetrieveOutcome) -> Option<BTreeMap<String, usize>> {
    out.entity_summary.clone()
}

fn serialized(out: &crate::types::RetrieveOutcome) -> Value {
    serde_json::to_value(out).expect("serialize outcome")
}

#[test]
fn the_toggle_is_on_by_default() {
    let store = fx::store();
    seed(&store, "bank");
    let out = fx::retriever(&store)
        .retrieve(&request("bank", &format!("{NAME} alpha")))
        .expect("retrieve");
    let hit = out
        .hits
        .iter()
        .find(|hit| hit.hit.item_id == "A")
        .expect("A hit");
    assert_eq!(names_of(hit), vec![NAME.to_owned()]);
    assert!(hit.entity_match, "the query mentions the name: {hit:?}");
    assert!(
        summary_of(&out).is_none(),
        "the summary is absent unless it was requested"
    );
}

#[test]
fn the_toggle_off_suppresses_every_name_on_the_page() {
    let store = fx::store();
    seed(&store, "bank");
    let retriever = fx::retriever(&store);
    let mut req = request("bank", &format!("{NAME} alpha"));
    req.limit = 5;
    req.include_entities = false;
    let out = retriever.retrieve(&req).expect("retrieve");

    assert_eq!(out.hits.len(), 3, "the page is otherwise unchanged");
    for hit in &out.hits {
        assert!(names_of(hit).is_empty(), "no name is published: {hit:?}");
        assert!(
            !hit.entity_match,
            "the derived flag has no name to compare: {hit:?}"
        );
    }
    // Every `entities` key stays present, so a consumer's parser never breaks.
    for hit in serialized(&out)["hits"].as_array().expect("hits") {
        assert_eq!(hit["entities"], json!([]), "{hit}");
    }
    assert!(
        summary_of(&out).is_none(),
        "a suppressed page carries no summary"
    );
}

#[test]
fn the_requested_summary_counts_names_across_the_page() {
    let store = fx::store();
    seed(&store, "bank");
    let mut req = request("bank", "alpha");
    req.limit = 5;
    req.entity_summary = true;
    let out = fx::retriever(&store).retrieve(&req).expect("retrieve");

    let summary = summary_of(&out).expect("summary present when requested");
    assert_eq!(summary.get(NAME), Some(&2), "two hits carry the name");
    assert_eq!(summary.get(OTHER), Some(&1));
    assert_eq!(summary.len(), 2, "no name is invented: {summary:?}");

    // BTreeMap key order makes the serialized object deterministic.
    let value = serialized(&out);
    let object = value["entity_summary"].as_object().expect("summary object");
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "keys are name-ascending: {value}");
    assert_eq!(object[NAME], json!(2), "{value}");
}

#[test]
fn the_summary_rolls_up_only_the_returned_page() {
    let store = fx::store();
    seed(&store, "bank");
    let mut req = request("bank", "alpha");
    req.limit = 1;
    req.entity_summary = true;
    let out = fx::retriever(&store).retrieve(&req).expect("retrieve");

    assert_eq!(out.hits.len(), 1, "the limit bounds the page");
    let summary = summary_of(&out).expect("summary present");
    let only = out
        .hits
        .first()
        .and_then(|hit| hit.entities.first())
        .expect("the returned hit names one entity");
    assert_eq!(summary.len(), 1, "only the returned hit is counted");
    assert_eq!(summary.get(only.as_str()), Some(&1));
}

#[test]
fn a_requested_summary_over_an_empty_page_is_an_empty_object() {
    let store = fx::store();
    seed(&store, "bank");
    let mut req = request("bank", "zzz-nothing-matches");
    req.entity_summary = true;
    let out = fx::retriever(&store).retrieve(&req).expect("retrieve");

    assert!(out.hits.is_empty(), "no hit matches the query");
    assert_eq!(
        summary_of(&out),
        Some(BTreeMap::new()),
        "an empty page summarizes to an empty object"
    );
    assert_eq!(
        serialized(&out)["entity_summary"],
        json!({}),
        "the key is present because it was requested"
    );
}

#[test]
fn a_summary_without_entity_names_is_rejected() {
    let store = fx::store();
    seed(&store, "bank");
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "alpha");
    req.include_entities = false;
    req.entity_summary = true;

    // The retriever rejects it, so even a direct in-process caller fails closed.
    let error = retriever
        .retrieve(&req)
        .expect_err("a summary without entity names is contradictory");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("entity_summary"), "{error}");

    // The in-process JSON surface reports the same rejection.
    let args = json!({
        "bank": "bank",
        "query": "alpha",
        "include_entities": false,
        "entity_summary": true
    });
    let value = serde_json::to_value(crate::surface::retrieve_from_json(&retriever, &args))
        .expect("serialize tool result");
    assert_eq!(value["ok"], json!(false), "{value}");
    assert_eq!(value["code"], json!("invalid_argument"), "{value}");
}

#[test]
fn a_name_reaches_only_the_hit_array_and_the_summary() {
    let store = fx::store();
    seed(&store, "bank");
    let mut req = request("bank", &format!("{NAME} alpha"));
    req.limit = 5;
    req.entity_summary = true;
    let out = fx::retriever(&store).retrieve(&req).expect("retrieve");
    assert!(!out.hits.is_empty(), "the page is not empty");

    let mut value = serialized(&out);
    for hit in value["hits"].as_array_mut().expect("hits") {
        let object = hit.as_object_mut().expect("object hit");
        assert!(object.remove("entities").is_some(), "entities key removed");
    }
    let object = value.as_object_mut().expect("outcome object");
    assert!(
        object.remove("entity_summary").is_some(),
        "summary key removed"
    );
    let rendered = value.to_string();
    assert!(!rendered.contains(NAME), "no name outside the two keys");
    assert!(!rendered.contains(OTHER), "no name outside the two keys");
}
