// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the per-hit `entities[]` array and the derived
//! `entity_match` flag at the MCP binding: the values the payload exposes,
//! their parity with the in-process surface, the documented empty coverage,
//! and the PII boundary that keeps entity names out of warnings, the
//! explanation trace, and the health surfaces.
//!
//! ## Owns
//! - `retrieve` payload `entities`/`entity_match` assertions and MCP/in-process parity
//! - The `retrieve` tool description's derived-status statement for the flag
//! - The "no entity name outside `hits[].entities`" boundary sweep
//! - `diagnose` / `verify` health-payload exclusion (entity names and the reason key)
//! - The "no `entity_match` outside the hit objects" diagnostic sweep
//!
//! ## Does not own
//! - The projection rules and trim semantics (`clio-retrieve` `entities`)
//! - CLI binding parity (`clio` read-entity tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the MCP entity contract and its PII boundary.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_retrieve::retrieve_from_json;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "ent-mcp";

/// A name no other field of any payload carries, so a boundary sweep that finds
/// it can only have found it in `hits[].entities`.
const ENTITY: &str = "EntBoundaryName77";

/// Hermetic state: no deployment overlay, so no embedder is resolved and the
/// test does not depend on a sidecar.
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

fn item(id: &str, snapshot: Option<Value>) -> MemoryItem {
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
        snapshot,
        // The gist carries the id so seeded items are distinct facts. A shared
        // gist would make them exact duplicates, and read-time collapse would
        // drop all but one before the shape assertions ran.
        gist: Some(format!("entity boundary note {id}")),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Persist one item with a lexical document so the lexical arm can return it.
fn put(state: &McpState, entry: &MemoryItem, text: &str) {
    state
        .store
        .create_memory_item(entry, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&entry.bank_id, &entry.id, text)
        .expect("lexical doc");
}

fn hits(payload: &Value) -> &[Value] {
    payload["hits"].as_array().expect("hits array")
}

fn hit_by_id<'a>(payload: &'a Value, id: &str) -> &'a Value {
    hits(payload)
        .iter()
        .find(|h| h["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))
}

/// The `entities` array of one hit, as a typed vector.
fn entities(hit: &Value) -> Vec<String> {
    let array = hit
        .get("entities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("entities key present: {hit}"));
    array
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("entity names are strings: {hit}"))
                .to_owned()
        })
        .collect()
}

/// The MCP payload exposes the verified snapshot entity
/// and the key is present even when the array is empty.
#[test]
fn mcp_payload_exposes_the_verified_snapshot_entity() {
    let state = state();
    put(
        &state,
        &item(
            "mcp-named",
            Some(json!({"entity": ENTITY, "amount": 1, "date": "2024-01-01"})),
        ),
        "entity boundary note",
    );
    put(&state, &item("mcp-plain", None), "entity boundary note");

    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "boundary", "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(hits(&out).len(), 2, "{out}");
    assert_eq!(
        entities(hit_by_id(&out, "mcp-named")),
        vec![ENTITY.to_owned()],
        "{out}"
    );
    assert_eq!(entities(hit_by_id(&out, "mcp-plain")), Vec::<String>::new());
}

/// The in-process surface exposes the same names for the same store
/// and the same arguments.
#[test]
fn mcp_and_in_process_surfaces_expose_identical_entities() {
    let state = state();
    put(
        &state,
        &item("par-named", Some(json!({"entity": ENTITY, "amount": 1}))),
        "entity boundary note",
    );
    put(&state, &item("par-plain", None), "entity boundary note");

    let args = json!({"bank": BANK, "query": "boundary", "limit": 5});
    let mcp = dispatch(&state, "retrieve", &args);
    let in_process = serde_json::to_value(retrieve_from_json(&state.retriever, &args))
        .expect("serialize outcome");
    assert_eq!(mcp["ok"], json!(true), "{mcp}");
    assert_eq!(in_process["ok"], json!(true), "{in_process}");

    for id in ["par-named", "par-plain"] {
        assert_eq!(
            entities(hit_by_id(&mcp, id)),
            entities(hit_by_id(&in_process, id)),
            "entity parity for `{id}`"
        );
    }
}

/// The documented empty-coverage shapes reach the MCP
/// payload as `[]`. The triple-carrier and hub-distill snapshot shapes are the
/// ones the write path stores (see the write crate's triple carrier and
/// hub-distill item builders).
#[test]
fn documented_empty_coverage_shapes_serialize_as_empty_arrays() {
    let state = state();
    let shapes: Vec<(&str, Option<Value>)> = vec![
        (
            "cov-triple",
            Some(json!({"subject": "rust", "predicate": "used_by", "object": "agent"})),
        ),
        (
            "cov-hub",
            Some(json!({"hub_id": "h", "neighbor_ids": [], "neighbor_count": 0})),
        ),
        ("cov-metric", Some(json!({"metric": "p95"}))),
        ("cov-number", Some(json!({"entity": 42}))),
        ("cov-blank", Some(json!({"entity": "  "}))),
        ("cov-none", None),
    ];
    for (id, snapshot) in &shapes {
        put(&state, &item(id, snapshot.clone()), "entity boundary note");
    }
    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "boundary", "limit": 10}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(hits(&out).len(), shapes.len(), "{out}");
    for hit in hits(&out) {
        assert_eq!(entities(hit), Vec::<String>::new(), "{hit}");
    }
}

/// The entity name appears only in `hits[].entities`. Every other
/// part of the payload — `warnings`, `explanation`, `metrics`, `dedup`, the
/// counts, and the frozen hit fields — is swept after the `entities` keys are
/// removed.
#[test]
fn entity_names_never_reach_warnings_or_the_explanation_trace() {
    let state = state();
    put(
        &state,
        &item("bnd-named", Some(json!({"entity": ENTITY, "amount": 1}))),
        "entity boundary note",
    );

    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "boundary", "explain": true, "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        out["explanation"].is_object(),
        "the explain trace is present: {out}"
    );
    assert!(out["warnings"].is_array(), "warnings present: {out}");
    assert_eq!(
        entities(hit_by_id(&out, "bnd-named")),
        vec![ENTITY.to_owned()],
        "the read payload carries the name: {out}"
    );

    let mut without_entities = out;
    for hit in without_entities["hits"]
        .as_array_mut()
        .expect("hits array")
        .iter_mut()
    {
        let object = hit.as_object_mut().expect("object hit");
        assert!(object.remove("entities").is_some(), "entities key removed");
    }
    let rendered = serde_json::to_string(&without_entities).expect("serialize payload");
    assert!(
        !rendered.contains(ENTITY),
        "no entity name outside hits[].entities: {rendered}"
    );
}

/// The operator health surfaces stay PII-safe, so a stored entity
/// name never appears in a diagnose or verify report. The same sweep asserts
/// the derived reason key: telemetry has no per-hit sink at all (every sink
/// reduces hits to the item id before serializing), so the health reports are
/// the only machine surfaces that could ever carry per-hit data, and this
/// assertion pins the reason's absence there too.
#[test]
fn entity_names_never_reach_the_health_surfaces() {
    let state = state();
    put(
        &state,
        &item("hlth-named", Some(json!({"entity": ENTITY, "amount": 1}))),
        "entity boundary note",
    );
    for name in ["diagnose", "verify"] {
        let out = dispatch(
            &state,
            name,
            &json!({"bank": BANK, "timestamp": "2024-01-01T00:00:00Z"}),
        );
        let rendered = serde_json::to_string(&out).expect("serialize report");
        assert!(
            !rendered.contains(ENTITY),
            "no entity name in the `{name}` report: {rendered}"
        );
        assert!(
            !rendered.contains("entity_match"),
            "no derived reason key in the `{name}` report: {rendered}"
        );
    }
}

/// The derived flag of one hit; a missing key means no match.
fn entity_match(hit: &Value) -> bool {
    hit.get("entity_match")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Only the hit whose own name is mentioned carries the
/// derived flag; the payload key is omitted for the others.
#[test]
fn mcp_payload_marks_only_the_query_matching_hit() {
    let state = state();
    put(
        &state,
        &item("em-named", Some(json!({"entity": ENTITY, "amount": 1}))),
        "entity boundary note",
    );
    put(&state, &item("em-plain", None), "entity boundary note");

    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": format!("{ENTITY} boundary"), "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(hits(&out).len(), 2, "{out}");
    let named = hit_by_id(&out, "em-named");
    assert!(entity_match(named), "the mentioned name is marked: {named}");
    assert_eq!(entities(named), vec![ENTITY.to_owned()], "{named}");
    let plain = hit_by_id(&out, "em-plain");
    assert!(!entity_match(plain), "no name, no flag: {plain}");
    assert!(
        plain.get("entity_match").is_none(),
        "the false flag is omitted: {plain}"
    );
}

/// The in-process surface projects the same flag for the same
/// store and arguments.
#[test]
fn mcp_and_in_process_surfaces_expose_identical_entity_match() {
    let state = state();
    put(
        &state,
        &item("mp-named", Some(json!({"entity": ENTITY}))),
        "entity boundary note",
    );
    put(&state, &item("mp-plain", None), "entity boundary note");

    let args = json!({"bank": BANK, "query": format!("{ENTITY} boundary"), "limit": 5});
    let mcp = dispatch(&state, "retrieve", &args);
    let in_process = serde_json::to_value(retrieve_from_json(&state.retriever, &args))
        .expect("serialize outcome");
    assert_eq!(mcp["ok"], json!(true), "{mcp}");
    assert_eq!(in_process["ok"], json!(true), "{in_process}");
    for id in ["mp-named", "mp-plain"] {
        assert_eq!(
            entity_match(hit_by_id(&mcp, id)),
            entity_match(hit_by_id(&in_process, id)),
            "entity_match parity for `{id}`"
        );
    }
    assert!(entity_match(hit_by_id(&mcp, "mp-named")));
    assert!(!entity_match(hit_by_id(&mcp, "mp-plain")));
}

/// The derived reason stays on the hit object. The explanation
/// trace and the rest of the payload carry no `entity_match` key: the trace
/// hit comes from the `ExplainHit` allowlist projection in `read_explain`,
/// and the exclusion is enforced by that projection's field list plus the
/// classification sweep in `read_explain_guard_tests`
/// (`every_hit_field_is_classified_and_matches_the_projection`).
#[test]
fn entity_match_never_reaches_the_explanation_trace() {
    let state = state();
    put(
        &state,
        &item("emx-named", Some(json!({"entity": ENTITY, "amount": 1}))),
        "entity boundary note",
    );
    let out = dispatch(
        &state,
        "retrieve",
        &json!({
            "bank": BANK,
            "query": format!("{ENTITY} boundary"),
            "explain": true,
            "limit": 5
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        out["explanation"].is_object(),
        "the explain trace is present: {out}"
    );
    assert!(entity_match(hit_by_id(&out, "emx-named")), "{out}");

    let mut without_hit_flags = out;
    for hit in without_hit_flags["hits"]
        .as_array_mut()
        .expect("hits array")
        .iter_mut()
    {
        let object = hit.as_object_mut().expect("object hit");
        object.remove("entity_match");
    }
    let rendered = serde_json::to_string(&without_hit_flags).expect("serialize payload");
    assert!(
        !rendered.contains("entity_match"),
        "no entity_match outside the hit objects: {rendered}"
    );
    for hit in without_hit_flags["explanation"]["hits"]
        .as_array()
        .expect("explanation hits")
    {
        assert!(
            hit.get("entity_match").is_none(),
            "no flag in the trace: {hit}"
        );
    }
}

/// The `retrieve` tool description states the derived status of the flag, so
/// the contract a machine consumer reads says that `entity_match` is a
/// display-only inference, never a verification result or a ranking signal.
#[test]
fn retrieve_tool_description_labels_entity_match_as_derived() {
    let def = crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "retrieve")
        .expect("retrieve tool definition");
    let description = def.description;
    assert!(
        description.contains("`entity_match`"),
        "the description names the field: {description}"
    );
    assert!(
        description.contains("derived display-only inference"),
        "the description marks the flag derived and display-only: {description}"
    );
    assert!(
        description.contains("not a verification result"),
        "the description denies verification status: {description}"
    );
    assert!(
        description.contains("not a ranking signal"),
        "the description denies ranking status: {description}"
    );
}
