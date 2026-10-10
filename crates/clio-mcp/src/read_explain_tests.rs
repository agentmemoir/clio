// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the wire-level explain-trace contract tests
//! at the MCP binding: the frozen trace key set and values, the PII boundary
//! that keeps content-derived fields out of the trace, and the key shape of a
//! hit that has no optional values.
//!
//! ## Owns
//! - The frozen trace key/value pin over the `retrieve` binding
//! - The entity-name boundary sweep over the trace
//! - The rendering boundary sweep over the trace: a snapshot-bearing hit keeps
//!   the frozen per-hit keys and no snapshot rendering text reaches the trace
//! - The absent-optional-fields trace shape, and the absent rendering key
//!
//! ## Does not own
//! - The projection's value contract and classification guard
//!   (`read_explain_guard_tests`)
//! - Retrieval behavior or hit construction (`clio-retrieve`)
//! - Transport parity masking (`mcp_read_transport_conformance`)
//! - The entity payload contract (`read_entities_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the wire-level explain-trace contract.

use crate::read_explain::TRACE_HIT_KEYS;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

const BANK: &str = "trace-mcp";

/// A name no other field of the trace payload carries, so a sweep that finds it
/// in the trace can only have found it in a content projection.
const ENTITY: &str = "TraceBoundaryName91";

/// The frozen top-level explain-trace keys for a plain (inert dedup) call.
const TRACE_TOP_LEVEL_KEYS: [&str; 10] = [
    "domains",
    "dense_candidates",
    "lexical_candidates",
    "fused_candidates",
    "reranked",
    "graph_expanded",
    "pairs_reinforced",
    "metrics",
    "warnings",
    "hits",
];

/// A set of key names for set-equality assertions.
fn set_of<const N: usize>(keys: [&str; N]) -> BTreeSet<String> {
    keys.map(String::from).into_iter().collect()
}

/// The object keys of one JSON object, as a set.
fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().expect("object").keys().cloned().collect()
}

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
        gist: Some("explain trace note".into()),
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

/// Two indexed facts: one entity-bearing, one snapshot-less.
fn seeded_state() -> Arc<McpState> {
    let state = state();
    put(
        &state,
        &item(
            "tr-named",
            Some(json!({"entity": ENTITY, "amount": 1, "date": "2024-01-01"})),
        ),
        "explain trace note",
    );
    put(&state, &item("tr-plain", None), "explain trace note");
    state
}

/// The explain trace of one payload.
fn trace(payload: &Value) -> &Value {
    payload
        .get("explanation")
        .expect("explain trace present on an `explain: true` call")
}

/// The payload hit with `id`, used to compare trace values against the read
/// payload they are derived from.
fn payload_hit<'a>(payload: &'a Value, id: &str) -> &'a Value {
    payload["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .find(|h| h["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("payload hit `{id}`: {payload}"))
}

/// The trace hit with `id`.
fn trace_hit<'a>(payload: &'a Value, id: &str) -> &'a Value {
    trace(payload)["hits"]
        .as_array()
        .expect("trace hits array")
        .iter()
        .find(|h| h["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("trace hit `{id}`: {payload}"))
}

/// An existing hit keeps the frozen trace key set and the values it derives
/// from the read payload, on both the top level and each hit.
#[test]
fn existing_hit_trace_keys_and_values_are_pinned() {
    let state = seeded_state();
    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "explain trace note", "explain": true, "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let trace = trace(&out);

    assert_eq!(
        keys(trace),
        set_of(TRACE_TOP_LEVEL_KEYS),
        "plain-call trace keys are frozen (inert dedup stays omitted): {trace}"
    );
    let trace_hits = trace["hits"].as_array().expect("trace hits");
    assert_eq!(trace_hits.len(), 2, "{out}");

    for hit in trace_hits {
        assert_eq!(
            keys(hit),
            set_of(TRACE_HIT_KEYS),
            "trace hits come from the shared projection allowlist: {hit}"
        );
        let id = hit["item_id"].as_str().expect("item id");
        let payload = payload_hit(&out, id);
        assert_eq!(
            hit["score"], payload["score"],
            "trace score mirrors the payload score: {hit}"
        );
        assert_eq!(
            hit["dense_rank"],
            json!(null),
            "no embedder means no dense rank: {hit}"
        );
        assert!(
            hit.get("temporal_rank").is_none(),
            "the frozen trace carries no temporal key, anchor or not: {hit}"
        );
        assert!(
            hit["lexical_rank"].as_u64().is_some_and(|r| r >= 1),
            "lexical rank is present: {hit}"
        );
        assert_eq!(hit["consolidated"], json!(false), "{hit}");
        assert_eq!(hit["source_ids"], json!([]), "{hit}");
    }

    assert_eq!(trace["domains"], json!([]), "{trace}");
    assert!(
        trace["pairs_reinforced"].as_u64().is_some(),
        "count present: {trace}"
    );
}

/// An entity-bearing hit keeps its name in the read payload but never in the
/// explain trace: no name, no content array, no gist, no context, no derived
/// reason flag.
#[test]
fn entity_bearing_hit_leaks_nothing_into_the_trace() {
    let state = seeded_state();
    let out = dispatch(
        &state,
        "retrieve",
        &json!({
            "bank": BANK,
            "query": format!("{ENTITY} explain trace note"),
            "explain": true,
            "limit": 5
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        payload_hit(&out, "tr-named")["entities"],
        json!([ENTITY]),
        "the read payload still carries the name: {out}"
    );

    let rendered = serde_json::to_string(trace(&out)).expect("serialize trace");
    assert!(
        !rendered.contains(ENTITY),
        "no entity name in the explain trace: {rendered}"
    );
    let hit = trace_hit(&out, "tr-named");
    assert!(
        hit.get("entities").is_none(),
        "no content array in the trace: {hit}"
    );
    assert!(hit.get("gist").is_none(), "no gist in the trace: {hit}");
    assert!(
        hit.get("context").is_none(),
        "no context in the trace: {hit}"
    );
    assert!(
        hit.get("entity_match").is_none(),
        "no derived reason flag in the trace: {hit}"
    );
}

/// The seeded entity-bearing item's snapshot, rendered the way a retrieval
/// hit's bounded rendering would render it. If this string reached the trace,
/// the rendering reached the trace.
fn seeded_snapshot_rendering() -> String {
    clio_retrieve::render_exact(&json!({"entity": ENTITY, "amount": 1, "date": "2024-01-01"}))
        .expect("the synthetic snapshot renders within the cap")
}

/// The rendering boundary sweep over the wire: a hit whose item carries a
/// snapshot must not put the snapshot's rendered text anywhere in the explain
/// trace, and every per-hit key set stays exactly the frozen allowlist.
///
/// `tr-named` carries a snapshot, so a rendering of that snapshot exists; the
/// string below is exactly what the bounded rendering carries. `tr-plain`
/// carries none, so none can. Both hits come back with the same frozen keys,
/// and neither trace holds any rendering text.
///
/// This sweep does not request a rendering, so it does not prove that a
/// populated rendering is excluded; the populated case is pinned at the
/// projection, where the fixture can carry the rendering unconditionally.
#[test]
fn a_snapshot_bearing_hit_leaks_no_rendering_into_the_trace() {
    let state = seeded_state();
    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "explain trace note", "explain": true, "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let rendering = seeded_snapshot_rendering();

    let trace_hits = trace(&out)["hits"].as_array().expect("trace hits").clone();
    assert_eq!(trace_hits.len(), 2, "{out}");
    for hit in &trace_hits {
        assert_eq!(
            keys(hit),
            set_of(TRACE_HIT_KEYS),
            "a hit keeps exactly the frozen per-hit keys whatever its item carries: {hit}"
        );
        assert!(
            hit.get("exact_render").is_none(),
            "no rendering key on a trace hit: {hit}"
        );
    }

    // The whole trace, not just each hit: the rendering must not reach the
    // counters, the metrics, or the warnings either.
    let serialized = serde_json::to_string(trace(&out)).expect("serialize trace");
    assert!(
        !serialized.contains(&rendering),
        "no snapshot rendering anywhere in the explain trace: {serialized}"
    );
    assert!(
        !serialized.contains(ENTITY),
        "no snapshot content anywhere in the explain trace: {serialized}"
    );
}

/// A read that did not ask for a rendering leaves the payload hit exactly as it
/// was before the field existed: no key at all, not `null`. The wire sweep
/// above pins the populated case; this one pins the default.
#[test]
fn a_hit_without_the_rendering_control_carries_no_rendering_key() {
    let state = seeded_state();
    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "explain trace note", "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    for hit in out["hits"].as_array().expect("hits") {
        assert!(
            hit.get("exact_render").is_none(),
            "a request that did not ask carries no rendering key: {hit}"
        );
    }
}

/// A hit without optional fields keeps the same trace key set, with
/// `null`/empty values instead of omitted keys.
#[test]
fn absent_optional_hit_fields_keep_the_trace_shape() {
    let state = seeded_state();
    let out = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "explain trace note", "explain": true, "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let hit = trace_hit(&out, "tr-plain");
    assert_eq!(
        hit,
        &json!({
            "item_id": "tr-plain",
            "score": payload_hit(&out, "tr-plain")["score"],
            "dense_rank": null,
            "lexical_rank": payload_hit(&out, "tr-plain")["lexical_rank"],
            "consolidated": false,
            "source_ids": []
        }),
        "a snapshot-less hit keeps every trace key: {hit}"
    );
}
