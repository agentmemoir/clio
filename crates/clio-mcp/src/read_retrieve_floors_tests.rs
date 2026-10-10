// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the optional `min_scores` score floors at the MCP
//! `retrieve` binding: the argument reaches the retriever with the documented
//! meaning, the MCP payload and the in-process surface agree for the same store
//! and arguments, a malformed floor is rejected with the documented code, and a
//! default call is unchanged.
//!
//! ## Owns
//! - `min_scores` binding assertions: MCP/in-process parity, arm-local `keyword`
//!   evidence, `final` no-backfill and empty-page evidence, the `reranker`
//!   no-op, and the rejection codes
//! - The default `retrieve` call's unchanged `entity_summary`/`entities` shape
//!
//! ## Does not own
//! - The floor rules, validation, and predicates (`clio-retrieve` `floors`,
//!   `floors_tests`, `floors_integration_tests`)
//! - The entity-inclusion controls (`read_entity_controls_tests`)
//! - The backend-dependence evidence for the live `ts_rank_cd` scale
//!   (`mcp_read_floors_postgres_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the MCP `min_scores` contract.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_index::{LexicalSearchArgs, lexical_search};
use clio_retrieve::MAX_FLOOR;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

const BANK: &str = "floors-mcp";

/// Two terms the query and both seeded documents share. The short document
/// matches them exactly while the long one carries filler, so FTS5's length
/// normalization makes the two raw lexical scores differ and a floor between
/// them can only drop the long one.
const QUERY: &str = "alpha beta";
const SHORT_DOC: &str = "alpha beta";
const LONG_DOC: &str = "alpha beta filler filler filler filler filler filler filler filler filler";

/// Hermetic state: an empty effective config resolves no embedder, so the dense
/// arm is off and the lexical arm alone scores the hits. That makes the
/// arm-local evidence readable: a hit the lexical arm produced reports
/// `scores.semantic: null` and is untouched by the dense arm's floor.
fn state() -> Arc<McpState> {
    let options = McpOpenOptions {
        default_bank: BANK.into(),
        ..McpOpenOptions::default()
    };
    McpState::open_with_effective(&options, &json!({})).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, args: Value) -> Value {
    dispatch(state, "retrieve", &args)
}

/// Run the same arguments through the MCP binding and through the in-process
/// surface, asserting both succeeded, so a comparison proves parity and
/// nothing else.
fn both_surfaces(state: &McpState, args: &Value) -> (Value, Value) {
    let mcp = dispatch(state, "retrieve", args);
    let raw = clio_retrieve::retrieve_from_json(&state.retriever, args);
    let in_process = serde_json::to_value(raw).expect("serialize outcome");
    assert_eq!(mcp["ok"], json!(true), "MCP surface: {mcp}");
    assert_eq!(
        in_process["ok"],
        json!(true),
        "in-process surface: {in_process}"
    );
    (mcp, in_process)
}

fn item(id: &str) -> MemoryItem {
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
        snapshot: Some(json!({"entity": id, "amount": 1})),
        gist: Some(format!("{id} memory note")),
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
fn put(state: &McpState, id: &str, text: &str) {
    let entry = item(id);
    state
        .store
        .create_memory_item(&entry, "agent")
        .expect("create item");
    let doc = state.index.put_lexical_doc(&entry.bank_id, &entry.id, text);
    doc.expect("lexical doc");
}

/// The fixture: one strong and one weak lexical candidate, both keeping a name.
fn seed_long_and_short(state: &McpState) {
    put(state, "SHORT", SHORT_DOC);
    put(state, "LONG", LONG_DOC);
}

fn hits(payload: &Value) -> &[Value] {
    payload["hits"].as_array().expect("hits array")
}

fn hit_ids(payload: &Value) -> Vec<String> {
    hits(payload)
        .iter()
        .filter_map(|hit| hit["item_id"].as_str().map(str::to_owned))
        .collect()
}

fn hit_by_id<'a>(payload: &'a Value, id: &str) -> &'a Value {
    hits(payload)
        .iter()
        .find(|hit| hit["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))
}

fn final_of(payload: &Value, id: &str) -> f64 {
    hit_by_id(payload, id)["scores"]["final"]
        .as_f64()
        .expect("numeric final")
}

/// The raw backend lexical score the lexical arm sees for one id, read through
/// the same `clio_index` facade retrieval uses, so the floor value comes from
/// the real score rather than a guess.
fn raw_keyword(state: &McpState, id: &str) -> f64 {
    let args = LexicalSearchArgs {
        bank: BANK,
        query: QUERY,
        domains: vec![],
        limit: 10,
    };
    let found = lexical_search(state.index.as_ref(), &args).expect("lexical search");
    let hit = found
        .iter()
        .find(|hit| hit.item_id == id)
        .unwrap_or_else(|| panic!("hit `{id}`"));
    hit.score
}

/// A `keyword` floor between the two raw lexical scores removes the weaker
/// lexical candidate and leaves the stronger one, and the survivor reports
/// `scores.semantic: null` because only the lexical arm ever scored it. That
/// null is the arm-local evidence: the dense arm never produced this candidate,
/// so no dense floor could have removed it and nothing about the dense arm's
/// score is being compared here.
#[test]
fn a_keyword_floor_prunes_the_lexical_arm_only() {
    let state = state();
    seed_long_and_short(&state);
    let strong = raw_keyword(&state, "SHORT");
    let weak = raw_keyword(&state, "LONG");
    assert!(
        strong > weak,
        "the fixture scores must differ: {strong} vs {weak}"
    );
    let floor = f64::midpoint(strong, weak);

    let out = call(
        &state,
        json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": {"keyword": floor}}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let ids = hit_ids(&out);
    assert!(ids.contains(&"SHORT".to_owned()), "{ids:?}");
    assert!(!ids.contains(&"LONG".to_owned()), "{ids:?}");

    let survivor = hit_by_id(&out, "SHORT");
    assert_eq!(
        survivor["scores"]["semantic"],
        json!(null),
        "only the lexical arm scored it, so no dense value exists: {survivor}"
    );
    assert_eq!(
        survivor["scores"]["keyword"],
        json!(strong),
        "the reported keyword is the raw backend value the floor compared: {survivor}"
    );
    assert!(
        survivor.get("dense_rank").is_none(),
        "the dense arm never ranked it: {survivor}"
    );

    // The unfloored call still returns both, so the floor is what changed.
    let unfloored = call(&state, json!({"bank": BANK, "query": QUERY, "limit": 5}));
    assert_eq!(unfloored["ok"], json!(true), "{unfloored}");
    assert_eq!(hit_ids(&unfloored).len(), 2, "{unfloored}");
}

/// The MCP payload and the in-process surface apply the same floor to the same
/// store and produce the same hit set and the same arm-local survivors, for a
/// `keyword` floor and for the `semantic` floor the lexical arm is unaffected
/// by.
#[test]
fn mcp_and_in_process_surfaces_apply_the_same_floors() {
    let state = state();
    seed_long_and_short(&state);
    let keyword_floor = f64::midpoint(raw_keyword(&state, "SHORT"), raw_keyword(&state, "LONG"));

    let (mcp, in_process) = both_surfaces(
        &state,
        &json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": {"keyword": keyword_floor}}),
    );
    assert_eq!(hit_ids(&mcp), hit_ids(&in_process), "same hit set: {mcp}");
    assert_eq!(
        hit_ids(&mcp),
        vec!["SHORT".to_owned()],
        "the floor removed the weaker candidate: {mcp}"
    );
    assert_eq!(
        hit_by_id(&mcp, "SHORT")["scores"]["keyword"],
        hit_by_id(&in_process, "SHORT")["scores"]["keyword"],
        "keyword parity: {mcp}"
    );
    assert_eq!(
        hit_by_id(&mcp, "SHORT")["scores"]["semantic"],
        json!(null),
        "the arm-local survivor reports null on both surfaces: {mcp}"
    );

    // A `semantic` floor of 1.0 prunes nothing, because the dense arm produced
    // no candidate at all. Both surfaces must agree on that no-op too.
    let (mcp_floored, in_process_floored) = both_surfaces(
        &state,
        &json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": {"semantic": 1.0}}),
    );
    assert_eq!(
        hit_ids(&mcp_floored),
        hit_ids(&in_process_floored),
        "a semantic floor is arm-local on both surfaces: {mcp_floored}"
    );
    assert_eq!(
        hit_ids(&mcp_floored).len(),
        2,
        "the dense arm is off, so its floor removes nothing: {mcp_floored}"
    );
}

/// A `final` floor between the two fused scores keeps only the stronger hit and
/// never backfills the freed slot: both candidates were already inside the fused
/// set and inside `limit`, so a floor that only removes candidates cannot
/// replace the dropped one. A floor nothing clears returns an empty page with
/// `ok: true`, which is the documented answer and not an error.
#[test]
fn a_final_floor_shortens_the_page_and_never_backfills() {
    let state = state();
    seed_long_and_short(&state);
    let unfloored = call(&state, json!({"bank": BANK, "query": QUERY, "limit": 5}));
    assert_eq!(unfloored["ok"], json!(true), "{unfloored}");
    let full_ids = hit_ids(&unfloored);
    assert_eq!(
        full_ids.len(),
        2,
        "both candidates are in the page: {unfloored}"
    );
    let (short_final, long_final) = (final_of(&unfloored, "SHORT"), final_of(&unfloored, "LONG"));
    let (best_id, best, weak) = if short_final >= long_final {
        ("SHORT", short_final, long_final)
    } else {
        ("LONG", long_final, short_final)
    };

    let floor = f64::midpoint(best, weak);
    let floored = call(
        &state,
        json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": {"final": floor}}),
    );
    assert_eq!(floored["ok"], json!(true), "{floored}");
    assert_eq!(
        hit_ids(&floored),
        vec![best_id.to_owned()],
        "only the stronger hit clears the floor, and the freed slot stays free: {floored}"
    );
    assert_eq!(
        floored["fused_candidates"], unfloored["fused_candidates"],
        "the floor filters the returned page, so fusion is unchanged: {floored}"
    );

    let unreachable = call(
        &state,
        json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": {"final": 1.0e6}}),
    );
    assert_eq!(
        unreachable["ok"],
        json!(true),
        "an empty page is not an error"
    );
    assert!(hits(&unreachable).is_empty(), "{unreachable}");
    assert_eq!(unreachable["fused_candidates"], json!(2), "{unreachable}");
}

/// With no reranker configured every hit's `scores.reranker` is `null`, so a
/// `reranker` floor has no value to compare and keeps every hit. It is a no-op,
/// not a filter that empties the page.
#[test]
fn a_reranker_floor_is_a_no_op_when_no_reranker_is_configured() {
    let state = state();
    seed_long_and_short(&state);
    let out = call(
        &state,
        json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": {"reranker": 0.99}}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["reranked"], json!(false), "no reranker ran: {out}");
    assert_eq!(hit_ids(&out).len(), 2, "the floor removed nothing: {out}");
    for hit in hits(&out) {
        assert_eq!(
            hit["scores"]["reranker"],
            json!(null),
            "the reranker never scored it, so there is nothing to compare: {hit}"
        );
    }
}

/// Every malformed or out-of-range `min_scores` is rejected with the documented
/// code, never silently accepted. A non-object shape, a non-numeric field, a
/// mistyped field name, and a negative value are `invalid_argument`;
/// `semantic`/`reranker` above 1.0 and `keyword`/`final` above `MAX_FLOOR` are
/// `out_of_range`. An in-range floor on the unbounded scales is accepted, so the
/// rejections are the documented rule and not a blanket refusal of the argument.
#[test]
fn malformed_floors_are_rejected_with_the_documented_codes() {
    let state = state();
    put(&state, "ANY", "alpha beta filler filler");
    let with =
        |floor: Value| json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": floor});
    for (floor, code) in [
        (json!("high"), "invalid_argument"),
        (json!({"final": "high"}), "invalid_argument"),
        (json!({"keyword": -1.0}), "invalid_argument"),
        // A mistyped name fails closed too, matching the schema's
        // `additionalProperties: false`, instead of being read as no floor.
        (json!({"semantic_typo": 0.5}), "invalid_argument"),
        (json!({"semantic": 1.5}), "out_of_range"),
        (json!({"reranker": 1.5}), "out_of_range"),
        (json!({"keyword": MAX_FLOOR + 1.0}), "out_of_range"),
        (json!({"final": MAX_FLOOR + 1.0}), "out_of_range"),
    ] {
        let out = call(&state, with(floor.clone()));
        assert_eq!(out["ok"], json!(false), "must be rejected: {floor} {out}");
        assert_eq!(out["code"], json!(code), "for {floor}: {out}");
    }
    // An in-range floor on the unbounded scale is accepted, so the rejections
    // above are the documented rule and not a blanket refusal of the argument.
    let in_range = call(&state, with(json!({"final": 0.0})));
    assert_eq!(in_range["ok"], json!(true), "in-range: {in_range}");
    assert_eq!(hit_ids(&in_range), vec!["ANY".to_owned()], "{in_range}");
}

/// A default call sets no floor: it keeps every lexical candidate, carries no
/// top-level `entity_summary` key, and every hit keeps its `entities` array. An
/// explicitly empty `min_scores` object is the same call.
#[test]
fn a_default_call_keeps_the_page_and_omits_the_summary() {
    let state = state();
    seed_long_and_short(&state);
    let out = call(&state, json!({"bank": BANK, "query": QUERY, "limit": 5}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        out.get("entity_summary").is_none(),
        "the summary is absent unless it is requested: {out}"
    );
    for hit in hits(&out) {
        assert_eq!(
            hit["entities"],
            json!([hit["item_id"].as_str().expect("id")]),
            "the default keeps each hit's names: {hit}"
        );
    }
    let empty = call(
        &state,
        json!({"bank": BANK, "query": QUERY, "limit": 5, "min_scores": {}}),
    );
    assert_eq!(empty["ok"], json!(true), "{empty}");
    assert_eq!(
        hit_ids(&empty),
        hit_ids(&out),
        "an empty object is a no-op: {empty}"
    );
}

/// The published `inputSchema` is the contract a caller reads before choosing a
/// value, so it states the arm-local reading, the inclusive bound, the
/// uncalibrated scales, the `keyword` backend dependence, and the no-backfill
/// rule for the `final` floor. The two entity toggles are asserted in
/// `read_entity_controls_tests`.
#[test]
fn the_published_schema_describes_the_floors() {
    let tool = crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "retrieve")
        .expect("retrieve tool definition");
    let floors = tool
        .props
        .iter()
        .find(|(key, _)| *key == "min_scores")
        .map(|(_, schema)| schema.clone())
        .expect("`min_scores` published");
    assert_eq!(floors["type"], json!("object"), "{floors}");
    assert!(floors.get("required").is_none(), "all optional: {floors}");
    let properties = floors["properties"].as_object().expect("floor properties");
    let names: Vec<&str> = properties.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        ["final", "keyword", "reranker", "semantic"],
        "{floors}"
    );
    for value in properties.values() {
        assert_eq!(
            value["type"],
            json!("number"),
            "each floor is numeric: {value}"
        );
    }
    let doc = floors.to_string();
    for (claim, needle) in [
        ("arm-local", "arm-local"),
        ("inclusive", "inclusive"),
        ("no backfill for the final floor", "never backfills"),
        ("uncalibrated", "never calibrated"),
        ("keyword backend dependence", "Backend-dependent"),
    ] {
        assert!(doc.contains(needle), "the schema states {claim}: {doc}");
    }
}
