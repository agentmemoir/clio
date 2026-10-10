// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the temporal-window retrieval binding at the MCP
//! `retrieve` and `compose_context` handlers: the anchor reaches the retriever,
//! the arm's counter and per-hit rank appear only when it ran, a malformed
//! anchor and an ambiguous phrase fail closed, bank scope holds, and the MCP
//! payload agrees with the in-process surface for the same store and arguments.
//!
//! ## Owns
//! - `query_anchor` binding assertions: the no-op default shape, the arm's
//!   `temporal_candidates` counter and per-hit `temporal_rank`, the
//!   `invalid_argument` rejection, the ambiguous-phrase no-op, and the
//!   bank-scope containment
//! - The compose memory section under the anchor
//! - The explain-trace counter for the arm (the trace's per-hit shape stays frozen)
//!
//! ## Does not own
//! - The window rules and phrase parsing (`clio-retrieve` `temporal`)
//! - The explain projection (`read_explain`) and the compose pack rules
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the MCP temporal binding.

use crate::read_explain::TRACE_HIT_KEYS;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

const BANK: &str = "temporal-mcp";
/// A second bank with an in-window item of its own, so scope can be proven.
const OTHER_BANK: &str = "temporal-mcp-other";
/// Noon UTC on Saturday 2026-09-26; `this week` is 2026-09-21..2026-09-28.
const ANCHOR: &str = "2026-09-26T12:00:00Z";
/// The query the window fixture is built around: exactly one supported phrase.
const WINDOW_QUERY: &str = "what happened this week";

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

/// The frozen top-level keys of a plain payload, including the dispatcher's
/// `ok` envelope flag (arm off, inert dedup).
const PLAIN_TOP_LEVEL_KEYS: [&str; 11] = [
    "ok",
    "hits",
    "dense_candidates",
    "lexical_candidates",
    "fused_candidates",
    "reranked",
    "graph_expanded",
    "pairs_reinforced",
    "estimated_tokens",
    "metrics",
    "warnings",
];

/// Every serialized key of the seeded snapshot-carrying fact on a no-anchor
/// call: the shape the parameter must not change.
const PLAIN_HIT_KEYS: [&str; 12] = [
    "item_id",
    "bank_id",
    "kind",
    "category",
    "epistemic_kind",
    "gist",
    "requires_confidence_check",
    "score",
    "scores",
    "lexical_rank",
    "snapshot_ref",
    "entities",
];

/// Hermetic state: an empty effective config resolves no embedder, so the
/// temporal arm is the only arm that can return the window items.
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

/// A semantic fact with an explicit validity interval.
fn item(id: &str, bank: &str, valid_from: &str, valid_until: Option<&str>) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: Some("turn-1".into()),
        snapshot: Some(json!({"entity": id, "amount": 1})),
        gist: Some(format!("{id} memory note")),
        valid_from: valid_from.into(),
        valid_until: valid_until.map(str::to_owned),
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Persist one fixture item with a non-matching lexical document, so only the
/// temporal arm can return it.
fn seed(state: &McpState, id: &str, bank: &str, from: &str, until: Option<&str>) {
    let entry = item(id, bank, from, until);
    state
        .store
        .create_memory_item(&entry, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&entry.bank_id, &entry.id, "unrelated")
        .expect("lexical doc");
}

/// Two in-window items (newest interval start first), one in-window item in
/// another bank, and one lexically matched item closed before the window.
fn seeded_state() -> Arc<McpState> {
    let state = state();
    for (id, bank, from, until) in [
        ("tw-later", BANK, "2026-09-25T00:00:00Z", None),
        ("tw-earlier", BANK, "2026-09-22T00:00:00Z", None),
        ("tw-other-bank", OTHER_BANK, "2026-09-25T00:00:00Z", None),
    ] {
        seed(&state, id, bank, from, until);
    }
    // The lexically matched item for the no-anchor shape test.
    let shape = item(
        "shape-item",
        BANK,
        "2024-01-01T00:00:00Z",
        Some("2025-01-01T00:00:00Z"),
    );
    state
        .store
        .create_memory_item(&shape, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(BANK, "shape-item", "shape note")
        .expect("lexical doc");
    state
}

fn hits(payload: &Value) -> &[Value] {
    payload["hits"].as_array().expect("hits array")
}

fn hit_ids(payload: &Value) -> Vec<&str> {
    hits(payload)
        .iter()
        .filter_map(|hit| hit["item_id"].as_str())
        .collect()
}

/// The object keys of one JSON object, as a set.
fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().expect("object").keys().cloned().collect()
}

/// A set of key names for set-equality assertions.
fn set_of<const N: usize>(names: [&str; N]) -> BTreeSet<String> {
    names.map(String::from).into_iter().collect()
}

/// The payload `retrieve` arguments for one window call, with `extra` merged in.
fn args(extra: &Value) -> Value {
    let mut base = json!({"bank": BANK, "query": WINDOW_QUERY, "limit": 5});
    let object = base.as_object_mut().expect("object args");
    for (key, value) in extra.as_object().expect("object extra") {
        object.insert(key.clone(), value.clone());
    }
    base
}

/// A plain call without an anchor keeps the exact top-level and per-hit key
/// sets it had before the parameter existed: no `temporal_candidates`, no
/// `temporal_rank`, and no other key that merely names the arm.
#[test]
fn a_plain_call_keeps_the_pre_anchor_shape() {
    let state = seeded_state();
    let out = call(
        &state,
        "retrieve",
        json!({"bank": BANK, "query": "shape note", "limit": 5}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        keys(&out),
        set_of(PLAIN_TOP_LEVEL_KEYS),
        "a no-anchor payload keeps its frozen top-level keys: {out}"
    );
    assert_eq!(hits(&out).len(), 1, "one lexical hit: {out}");
    let hit = &hits(&out)[0];
    assert_eq!(hit["item_id"], json!("shape-item"), "{out}");
    assert_eq!(
        keys(hit),
        set_of(PLAIN_HIT_KEYS),
        "a no-anchor hit keeps its frozen keys: {out}"
    );
    let rendered = serde_json::to_string(&out).expect("serialize payload");
    for key in ["temporal_candidates", "temporal_rank"] {
        assert!(!rendered.contains(key), "no `{key}` anywhere: {rendered}");
    }
}

/// With an explicit anchor and one supported phrase the arm runs: the counter
/// is present and each in-window hit carries its 1-based arm rank in the arm's
/// own order (interval start descending).
#[test]
fn the_anchor_runs_the_arm_and_ranks_its_hits() {
    let state = seeded_state();
    let out = call(&state, "retrieve", args(&json!({"query_anchor": ANCHOR})));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        out["temporal_candidates"],
        json!(2),
        "only the two in-window items are candidates: {out}"
    );
    assert_eq!(
        hit_ids(&out),
        vec!["tw-later", "tw-earlier"],
        "newest interval start first: {out}"
    );
    let ranks: Vec<&Value> = hits(&out).iter().map(|hit| &hit["temporal_rank"]).collect();
    assert_eq!(ranks, vec![&json!(1), &json!(2)], "{out}");
    for hit in hits(&out) {
        assert_eq!(hit["bank_id"], json!(BANK), "{hit}");
    }
}

/// A malformed anchor is rejected with the documented code even before any arm
/// could run, and the rejection comes from the shared tool, not the binding.
#[test]
fn a_malformed_anchor_is_rejected_as_invalid_argument() {
    let state = seeded_state();
    for anchor in ["2026-09-26", "2026-09-26T12:00:00"] {
        let out = call(&state, "retrieve", args(&json!({"query_anchor": anchor})));
        assert_eq!(out["ok"], json!(false), "must be rejected: {anchor} {out}");
        assert_eq!(out["code"], json!("invalid_argument"), "{anchor}: {out}");
    }
}

/// Two different phrases in one query resolve to two windows, so the arm never
/// guesses and the call is a plain no-op: no counter, no ranks, no hits.
#[test]
fn an_ambiguous_phrase_runs_no_arm() {
    let state = seeded_state();
    let out = call(
        &state,
        "retrieve",
        json!({
            "bank": BANK,
            "query": "today or this week",
            "query_anchor": ANCHOR,
            "limit": 5
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out.get("temporal_candidates").is_none(), "{out}");
    assert!(hits(&out).is_empty(), "{out}");
}

/// The trace keeps its frozen top level plus the counter only when the arm ran;
/// each hit's arm rank stays on the payload, never in the trace.
#[test]
fn the_explain_trace_carries_the_arm_counter_and_rank() {
    let state = seeded_state();
    let out = call(
        &state,
        "retrieve",
        args(&json!({"query_anchor": ANCHOR, "explain": true})),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let explanation = out
        .get("explanation")
        .expect("explain trace present on an `explain: true` call");
    let mut expected = set_of(TRACE_TOP_LEVEL_KEYS);
    expected.insert("temporal_candidates".into());
    assert_eq!(keys(explanation), expected, "{explanation}");
    assert_eq!(explanation["temporal_candidates"], json!(2), "{out}");
    let trace_hits = explanation["hits"].as_array().expect("trace hits");
    assert_eq!(trace_hits.len(), 2, "{out}");
    for hit in trace_hits {
        assert_eq!(
            keys(hit),
            set_of(TRACE_HIT_KEYS),
            "trace hits come from the shared projection allowlist: {hit}"
        );
    }
    let ranks: Vec<&Value> = hits(&out).iter().map(|hit| &hit["temporal_rank"]).collect();
    assert_eq!(ranks, vec![&json!(1), &json!(2)], "{out}");

    // A plain call's trace stays at the 10 frozen keys.
    let plain = call(
        &state,
        "retrieve",
        json!({"bank": BANK, "query": WINDOW_QUERY, "explain": true}),
    );
    assert_eq!(plain["ok"], json!(true), "{plain}");
    let plain_trace = plain.get("explanation").expect("trace present");
    assert_eq!(
        keys(plain_trace),
        set_of(TRACE_TOP_LEVEL_KEYS),
        "no counter for a no-anchor call: {plain_trace}"
    );
}

/// Drop the wall-clock fields; the clock-derived `scores.age_factor` is snapped
/// to a 1e-12 grid so a one-ULP difference vanishes but a real disagreement survives.
fn strip_volatile(mut value: Value) -> Value {
    value
        .as_object_mut()
        .expect("payload object")
        .remove("metrics");
    for hit in value["hits"].as_array_mut().expect("hits array") {
        let object = hit.as_object_mut().expect("hit object");
        object.remove("score");
        if let Some(scores) = object.get_mut("scores").and_then(Value::as_object_mut) {
            scores.remove("final");
            if let Some(f) = scores.get("age_factor").and_then(Value::as_f64) {
                scores.insert("age_factor".into(), ((f * 1e12).round() / 1e12).into());
            }
        }
    }
    value
}

/// The MCP payload and the in-process surface produce the same full response
/// for the same store and the same anchored arguments, apart from the wall-clock fields.
#[test]
fn mcp_and_in_process_surfaces_agree_when_the_arm_runs() {
    let state = seeded_state();
    let args = args(&json!({"query_anchor": ANCHOR}));
    let mcp = call(&state, "retrieve", args.clone());
    let in_process =
        serde_json::to_value(clio_retrieve::retrieve_from_json(&state.retriever, &args))
            .expect("serialize in-process outcome");
    assert_eq!(mcp["ok"], json!(true), "MCP surface: {mcp}");
    assert_eq!(
        in_process["ok"],
        json!(true),
        "in-process surface: {in_process}"
    );
    assert_eq!(
        strip_volatile(mcp),
        strip_volatile(in_process),
        "the two surfaces agree on the whole payload apart from clock-derived numbers"
    );
}

/// The window read is bank-scoped: an in-window item in another bank is not a
/// candidate and never reaches the page.
#[test]
fn the_window_read_is_bank_scoped() {
    let state = seeded_state();
    let out = call(
        &state,
        "retrieve",
        args(&json!({"query_anchor": ANCHOR, "limit": 10})),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        out["temporal_candidates"],
        json!(2),
        "the other bank's in-window item is not counted: {out}"
    );
    assert_eq!(hit_ids(&out), vec!["tw-later", "tw-earlier"], "{out}");
    let rendered = serde_json::to_string(&out).expect("serialize payload");
    assert!(
        !rendered.contains("tw-other-bank"),
        "no other-bank id in the payload: {rendered}"
    );
}

/// `compose_context` binds the same anchor for its memory section: the
/// in-window items are packed with it and are not packed without it.
#[test]
fn the_compose_memory_section_honors_the_anchor() {
    let state = seeded_state();
    // The persona section is first, the memory section second.
    let memory = |payload: &Value| -> Value {
        assert_eq!(payload["sections"][1]["name"], json!("memory"), "{payload}");
        payload["sections"][1]["item_ids"].clone()
    };

    let with_arm = call(
        &state,
        "compose_context",
        json!({
            "bank": BANK,
            "query": WINDOW_QUERY,
            "query_anchor": ANCHOR,
            "budget_tokens": 512
        }),
    );
    assert_eq!(with_arm["ok"], json!(true), "{with_arm}");
    assert_eq!(
        memory(&with_arm),
        json!(["tw-later", "tw-earlier"]),
        "the anchored memory section packs the in-window items: {with_arm}"
    );

    let without_arm = call(
        &state,
        "compose_context",
        json!({"bank": BANK, "query": WINDOW_QUERY, "budget_tokens": 512}),
    );
    assert_eq!(without_arm["ok"], json!(true), "{without_arm}");
    assert_eq!(
        memory(&without_arm),
        json!([]),
        "without the anchor the relative query contributes nothing: {without_arm}"
    );
}
