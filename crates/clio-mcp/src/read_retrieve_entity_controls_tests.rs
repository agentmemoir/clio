// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the two entity-inclusion controls at the MCP `retrieve`
//! binding: the `include_entities` toggle on the per-hit `entities[]` array and
//! the derived `entity_match` flag, and the requested top-level `entity_summary`
//! object, including the content boundary both of them keep.
//!
//! ## Owns
//! - `include_entities: false` emptying `entities` and dropping `entity_match`,
//!   and the default keeping both
//! - `entity_summary: true` counting, name-ascending key order, the page-only
//!   roll-up, the empty-page object, and the rejection with `include_entities: false`
//! - The boundary sweep proving a stored name reaches only `hits[].entities`
//!   and the top-level `entity_summary`, never `warnings` or the `explain` trace
//!
//! ## Does not own
//! - The projection rules, the overlap match rule, and the toggle/summary
//!   consistency rule (`clio-retrieve` `entities`, `entity_match`, `entity_summary`)
//! - The per-hit entity contract without a toggle (`read_entities_tests`)
//! - The MCP-level health-surface sweep for entity names (`read_entities_tests`)
//! - The score floors on the same tool (`read_floors_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the MCP entity controls and their content boundary.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "entctl-mcp";

/// A name no other payload field carries, so a boundary sweep that finds it can
/// only have found it in a hit's `entities` array or in the summary.
const NAME: &str = "EntToggleMcpName42";
const OTHER: &str = "EntToggleMcpOther17";

/// Hermetic state: an empty effective config resolves no embedder, so the
/// lexical arm alone produces the page and no sidecar timing enters the tests.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: BANK.into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// A semantic item whose verified snapshot carries one entity name.
fn named_item(id: &str, entity: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: Some("turn-1".into()),
        snapshot: Some(json!({"entity": entity, "amount": 1})),
        // The gist carries the id so seeded items are distinct facts. A shared
        // gist would make them exact duplicates, and read-time collapse would
        // drop all but one before the entity assertions ran.
        gist: Some(format!("entity control note {id}")),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Persist one item with a lexical document so the lexical arm returns it.
fn put(state: &McpState, id: &str, entity: &str) {
    let entry = named_item(id, entity);
    state
        .store
        .create_memory_item(&entry, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&entry.bank_id, &entry.id, "alpha entity control token")
        .expect("lexical doc");
}

/// Two items sharing `NAME` and one carrying `OTHER`, so the page has a name
/// that repeats and a name that does not.
fn seed(state: &McpState) {
    for (id, entity) in [("A", NAME), ("B", NAME), ("C", OTHER)] {
        put(state, id, entity);
    }
}

fn hits(payload: &Value) -> &[Value] {
    payload["hits"].as_array().expect("hits array")
}

fn hit_by_id<'a>(payload: &'a Value, id: &str) -> &'a Value {
    hits(payload)
        .iter()
        .find(|hit| hit["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))
}

/// The default `retrieve` arguments for this bank, with `extra` merged in.
fn args(extra: &Value) -> Value {
    let mut base = json!({"bank": BANK, "query": "alpha", "limit": 5});
    let object = base.as_object_mut().expect("object args");
    for (key, value) in extra.as_object().expect("object extra") {
        object.insert(key.clone(), value.clone());
    }
    base
}

/// The derived flag of one hit; a missing key means no match.
fn entity_match(hit: &Value) -> bool {
    hit.get("entity_match")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// `include_entities: false` empties every hit's `entities` array and drops the
/// derived `entity_match` key, while the page itself is unchanged. No entity
/// name is left anywhere in the response.
#[test]
fn the_toggle_off_empties_entities_and_drops_the_derived_flag() {
    let state = state();
    seed(&state);

    let out = call(
        &state,
        "retrieve",
        args(&json!({"include_entities": false, "query": format!("{NAME} alpha")})),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        hits(&out).len(),
        3,
        "the page is otherwise unchanged: {out}"
    );
    for hit in hits(&out) {
        assert_eq!(
            hit["entities"],
            json!([]),
            "the key stays present and empty so a parser never breaks: {hit}"
        );
        assert!(
            hit.get("entity_match").is_none(),
            "there is no name to derive the flag from: {hit}"
        );
    }
    let rendered = serde_json::to_string(&out).expect("serialize payload");
    assert!(!rendered.contains(NAME), "no name anywhere: {rendered}");
    assert!(!rendered.contains(OTHER), "no name anywhere: {rendered}");
}

/// The default call keeps the per-hit array and the derived flag exactly as
/// before these controls existed, and never carries a summary key.
#[test]
fn the_default_call_keeps_entities_and_the_derived_flag() {
    let state = state();
    seed(&state);

    let out = call(
        &state,
        "retrieve",
        args(&json!({"query": format!("{NAME} alpha")})),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let named = hit_by_id(&out, "A");
    assert_eq!(named["entities"], json!([NAME]), "{named}");
    assert!(entity_match(named), "the query mentions the name: {named}");
    let other = hit_by_id(&out, "C");
    assert_eq!(other["entities"], json!([OTHER]), "{other}");
    assert!(
        !entity_match(other),
        "the query never mentions this name: {other}"
    );
    assert!(
        out.get("entity_summary").is_none(),
        "the summary is absent unless it is requested: {out}"
    );
}

/// `entity_summary: true` returns the counted object with name-ascending keys,
/// and a name carried by more than one returned hit counts every one of them.
#[test]
fn the_requested_summary_counts_a_shared_name_on_every_hit() {
    let state = state();
    seed(&state);

    let out = call(&state, "retrieve", args(&json!({"entity_summary": true})));
    assert_eq!(out["ok"], json!(true), "{out}");
    let summary = out["entity_summary"].as_object().expect("summary object");
    assert_eq!(summary[NAME], json!(2), "two returned hits carry it: {out}");
    assert_eq!(summary[OTHER], json!(1), "{out}");
    assert_eq!(summary.len(), 2, "no name is invented: {out}");
    let keys: Vec<&str> = summary.keys().map(String::as_str).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "keys are name-ascending: {out}");
}

/// The summary is a roll-up of the returned page, not of the whole bank: with
/// `limit: 1` it carries exactly the one name the returned hit has.
#[test]
fn the_summary_rolls_up_only_the_returned_page() {
    let state = state();
    seed(&state);

    let out = call(
        &state,
        "retrieve",
        args(&json!({"entity_summary": true, "limit": 1})),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(hits(&out).len(), 1, "the limit bounds the page: {out}");
    let only = &hits(&out)[0];
    let only_name = only["entities"][0]
        .as_str()
        .expect("the returned hit names an entity");
    let summary = out["entity_summary"].as_object().expect("summary object");
    assert_eq!(summary.len(), 1, "only the returned hit is counted: {out}");
    assert_eq!(summary[only_name], json!(1), "{out}");
}

/// A requested summary over an empty page is an empty object, and the key is
/// present because it was asked for.
#[test]
fn a_requested_summary_over_an_empty_page_is_an_empty_object() {
    let state = state();
    seed(&state);

    let out = call(
        &state,
        "retrieve",
        args(&json!({"entity_summary": true, "query": "zzz-nothing-matches"})),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(hits(&out).is_empty(), "no hit matches: {out}");
    assert_eq!(out["entity_summary"], json!({}), "{out}");
}

/// A summary asked for together with `include_entities: false` is rejected with
/// `invalid_argument` rather than silently dropping one of the two.
#[test]
fn a_summary_without_entity_names_is_rejected() {
    let state = state();
    seed(&state);

    let out = call(
        &state,
        "retrieve",
        args(&json!({"include_entities": false, "entity_summary": true})),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
    assert!(
        out["message"]
            .as_str()
            .is_some_and(|message| message.contains("entity_summary")),
        "the message names the contradiction: {out}"
    );

    // The in-process surface rejects the same combination, so the rule is not
    // a binding-local second check.
    let in_process = serde_json::to_value(clio_retrieve::retrieve_from_json(
        &state.retriever,
        &json!({
            "bank": BANK,
            "query": "alpha",
            "include_entities": false,
            "entity_summary": true
        }),
    ))
    .expect("serialize tool result");
    assert_eq!(in_process["ok"], json!(false), "{in_process}");
    assert_eq!(
        in_process["code"],
        json!("invalid_argument"),
        "{in_process}"
    );
}

/// The content boundary at this binding: with a summary requested and the trace
/// on, a stored entity name appears nowhere except `hits[].entities` and the
/// top-level `entity_summary`. Removing those two keys leaves no name anywhere,
/// including `warnings` and `explanation`.
#[test]
fn entity_names_reach_only_the_hit_array_and_the_summary() {
    let state = state();
    seed(&state);

    let out = call(
        &state,
        "retrieve",
        args(&json!({"entity_summary": true, "query": format!("{NAME} alpha"), "explain": true})),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        out["explanation"].is_object(),
        "the explain trace is present: {out}"
    );
    assert!(out["warnings"].is_array(), "warnings present: {out}");
    assert_eq!(out["entity_summary"][NAME], json!(2), "{out}");

    let mut stripped = out;
    for hit in stripped["hits"]
        .as_array_mut()
        .expect("hits array")
        .iter_mut()
    {
        let object = hit.as_object_mut().expect("object hit");
        assert!(object.remove("entities").is_some(), "entities key removed");
    }
    assert!(
        stripped
            .as_object_mut()
            .expect("outcome object")
            .remove("entity_summary")
            .is_some(),
        "summary key removed"
    );
    let rendered = serde_json::to_string(&stripped).expect("serialize payload");
    assert!(
        !rendered.contains(NAME),
        "no name outside hits[].entities and entity_summary: {rendered}"
    );
    assert!(
        !rendered.contains(OTHER),
        "no name outside hits[].entities and entity_summary: {rendered}"
    );
}

/// The published `inputSchema` states each toggle's default and effect, states
/// the contradiction between the two, and keeps entity names out of the
/// warnings, the trace, telemetry, and the health surfaces.
#[test]
fn the_published_schema_describes_the_entity_controls() {
    let tool = crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "retrieve")
        .expect("retrieve tool definition");
    let prop = |name: &str| {
        let schema = tool
            .props
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, schema)| schema.clone());
        schema.unwrap_or_else(|| panic!("`{name}` published"))
    };
    let toggle = prop("include_entities");
    assert_eq!(toggle["type"], json!("boolean"), "{toggle}");
    let summary = prop("entity_summary");
    assert_eq!(summary["type"], json!("boolean"), "{summary}");
    assert!(
        !tool.required.contains(&"include_entities") && !tool.required.contains(&"entity_summary"),
        "both toggles are optional: {tool:?}"
    );
    for (claim, doc) in [
        ("include default true", toggle.to_string()),
        ("summary default false", summary.to_string()),
    ] {
        assert!(doc.contains("default"), "the schema states {claim}: {doc}");
    }
    for surface in [
        "`warnings`",
        "`explain` trace",
        "telemetry",
        "health surfaces",
    ] {
        assert!(
            summary.to_string().contains(surface),
            "the summary excludes {surface}: {summary}"
        );
    }
    assert!(
        summary.to_string().contains("include_entities: false"),
        "the summary states the contradiction: {summary}"
    );
}
