// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP read-surface scope/dedup tests:
//! bank-default vs call override, explicit-call execution under a skippable
//! gate, transport-neutral protocol parity, cross-bank isolation, the schema
//! publication of the new parameters, and byte-identical default output.
//!
//! ## Owns
//! - `recall_scope` / `prefer_consolidated` handler assertions and fixtures
//!
//! ## Does not own
//! - Dedup ranking internals (`clio-retrieve`) or the generic read matrix
//!   (`read_tools_tests`, `mcp_read_conformance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::read_scores_test_support::strip_volatile;
use crate::protocol::McpHandler;
use crate::runtime::{McpOpenOptions, McpState};
use crate::runtime_retention::retention_test_support::RetentionStoreFixture;
use crate::schema::schema_pack;
use crate::write_tools::dispatch;
use clio_config::retention::{RecallScope, RetentionProfile};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};

fn state() -> RetentionStoreFixture {
    // Hermetic: `open` layers the machine's deployment file, which turns
    // these parity tests into timing flakes where sidecars live. The retention
    // store is redirected to a file this fixture owns.
    RetentionStoreFixture::open_with_effective(
        &McpOpenOptions {
            default_bank: "bank-a".into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn base_item(id: &str, bank: &str, gist: &str) -> MemoryItem {
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
        snapshot: Some(json!({"entity": gist})),
        gist: Some(gist.into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Hub-distilled consolidated parent claiming `hub` + `neighbors`.
fn parent_item(id: &str, bank: &str, hub: &str, neighbors: &[&str], gist: &str) -> MemoryItem {
    let mut item = base_item(id, bank, gist);
    item.source_ref = Some(clio_retrieve::HUB_DISTILL_SOURCE_REF.into());
    item.snapshot = Some(json!({
        "hub_id": hub,
        "neighbor_ids": neighbors,
        "neighbor_count": neighbors.len(),
    }));
    item
}

fn put(state: &McpState, item: &MemoryItem, text: &str) {
    state
        .store
        .create_memory_item(item, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&item.bank_id, &item.id, text)
        .expect("lexical doc");
}

/// Parent + two raw sources + one unrelated item, all lexically queryable.
fn duplicate_bank(state: &McpState, bank: &str) {
    put(
        state,
        &parent_item("P", bank, "HUB", &["A", "B"], "alpha bravo charlie delta"),
        "alpha bravo charlie delta",
    );
    put(
        state,
        &base_item("A", bank, "alpha bravo charlie echo"),
        "alpha bravo charlie echo",
    );
    put(
        state,
        &base_item("B", bank, "alpha bravo charlie foxtrot"),
        "alpha bravo charlie foxtrot",
    );
    put(
        state,
        &base_item("U", bank, "alpha unrelated hotel india"),
        "alpha unrelated hotel india",
    );
}

fn hit_ids(out: &Value) -> Vec<String> {
    out["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["item_id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn t27_05_bank_default_scope_is_honored_and_call_overrides() {
    let s = state();
    duplicate_bank(&s, "bank-a");
    s.set_retention(
        "bank-a",
        RetentionProfile {
            recall_scope_default: RecallScope::ConsolidatedOnly,
            ..RetentionProfile::default()
        },
    )
    .expect("store profile");

    // Call omits recall_scope: the bank default (consolidated_only) applies.
    let defaulted = call(&s, "retrieve", json!({"bank": "bank-a", "query": "alpha"}));
    assert_eq!(defaulted["ok"], json!(true), "{defaulted}");
    assert_eq!(hit_ids(&defaulted), vec!["P".to_owned()]);
    assert_eq!(
        defaulted["dedup"]["scope_applied"],
        json!("consolidated_only")
    );

    // Explicit call parameter always wins over the bank default.
    let explicit = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "alpha", "recall_scope": "full"}),
    );
    assert!(hit_ids(&explicit).len() > 1, "{explicit}");

    // An unknown scope value is rejected, never silently widened.
    let bad = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "alpha", "recall_scope": "observations"}),
    );
    assert_eq!(bad["ok"], json!(false));
    assert_eq!(bad["code"], json!("invalid_argument"));
}

#[test]
fn t27_08_explicit_scoped_call_runs_when_the_gate_would_skip() {
    let s = state();
    duplicate_bank(&s, "bank-a");
    let gate = call(&s, "intent_gate", json!({"turn_text": "hi"}));
    assert_eq!(gate["retrieval_needed"], json!(false), "{gate}");

    let out = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "alpha", "recall_scope": "consolidated_only"}),
    );
    assert_eq!(out["ok"], json!(true), "explicit call must run: {out}");
    assert_eq!(hit_ids(&out), vec!["P".to_owned()]);
    assert_eq!(out["dedup"]["scope_applied"], json!("consolidated_only"));
}

#[test]
fn t27_09_scope_and_dedup_never_cross_banks() {
    let s = state();
    // bank-b's parent claims "shared-A", an id that exists only in bank-a.
    // A cross-bank leak would let that parent suppress bank-a's raw.
    put(
        &s,
        &parent_item(
            "Pb",
            "bank-b",
            "HUbb",
            &["shared-A", "raw-b"],
            "bank b parent",
        ),
        "alpha bravo charlie delta",
    );
    put(
        &s,
        &base_item("raw-b", "bank-b", "alpha bravo charlie echo"),
        "alpha bravo charlie echo",
    );
    put(
        &s,
        &base_item("U-b", "bank-b", "alpha unrelated hotel india"),
        "alpha unrelated hotel india",
    );
    put(
        &s,
        &base_item("shared-A", "bank-a", "alpha bravo charlie echo"),
        "alpha bravo charlie echo",
    );

    // bank-b: its own returned parent suppresses raw-b; U-b stays.
    let bank_b = call(
        &s,
        "retrieve",
        json!({"bank": "bank-b", "query": "alpha", "prefer_consolidated": true}),
    );
    assert_eq!(bank_b["ok"], json!(true), "{bank_b}");
    let mut returned = hit_ids(&bank_b);
    returned.sort();
    assert_eq!(returned, vec!["Pb".to_owned(), "U-b".into()]);
    assert_eq!(bank_b["dedup"]["raw_suppressed"], json!(1));
    assert_eq!(bank_b["dedup"]["suppressed_raw_ids"], json!(["raw-b"]));

    // bank-a: no parent of its own, so nothing suppresses shared-A. The inert
    // report is omitted; a leaked bank-b parent would have changed both.
    let bank_a = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "alpha", "prefer_consolidated": true}),
    );
    assert_eq!(bank_a["ok"], json!(true), "{bank_a}");
    assert_eq!(hit_ids(&bank_a), vec!["shared-A".to_owned()]);
    assert!(
        bank_a.get("dedup").is_none(),
        "no cross-bank suppression: {bank_a}"
    );
}

#[test]
fn prefer_consolidated_suppresses_raws_and_reports_counters() {
    let s = state();
    duplicate_bank(&s, "bank-a");
    let out = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "alpha", "prefer_consolidated": true, "explain": true}),
    );
    let returned = hit_ids(&out);
    assert!(returned.contains(&"P".to_owned()), "{out}");
    assert!(
        !returned.contains(&"A".to_owned()),
        "raw source suppressed: {out}"
    );
    assert!(
        !returned.contains(&"B".to_owned()),
        "raw source suppressed: {out}"
    );
    assert_eq!(out["dedup"]["raw_suppressed"], json!(2));
    assert_eq!(out["dedup"]["suppressed_raw_ids"], json!(["A", "B"]));
    // Explain traces expose ids and counters only, never candidate text.
    let explanation = &out["explanation"]["dedup"];
    assert_eq!(explanation["scope_applied"], json!("full"));
    assert!(explanation.get("suppressed_raw_ids").is_some());
    assert!(
        explanation["backfilled"].as_u64().is_some(),
        "backfill counter always present: {explanation}"
    );
}

#[test]
fn plain_retrieve_payload_keeps_the_prior_shape() {
    let s = state();
    duplicate_bank(&s, "bank-a");
    let out = call(&s, "retrieve", json!({"bank": "bank-a", "query": "alpha"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        out.get("dedup").is_none(),
        "no new params must stay byte-identical: {out}"
    );
    assert_eq!(
        hit_ids(&out).len(),
        4,
        "no suppression without the new params"
    );

    let explained = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "alpha", "explain": true}),
    );
    assert!(
        explained["explanation"].is_object(),
        "explain still returns a trace: {explained}"
    );
    assert!(
        explained.get("dedup").is_none(),
        "no top-level dedup without the new params: {explained}"
    );
    assert!(
        explained["explanation"].get("dedup").is_none(),
        "inert dedup report omitted from the trace: {explained}"
    );
}

#[test]
fn compose_explanation_reports_dedup_only_for_a_non_inert_call() {
    let s = state();
    duplicate_bank(&s, "bank-a");
    let plain = call(
        &s,
        "compose_context",
        json!({"bank": "bank-a", "query": "alpha", "budget_tokens": 256, "explain": true}),
    );
    assert_eq!(plain["ok"], json!(true), "{plain}");
    assert!(
        plain["explanation"].is_object(),
        "explain still returns a trace: {plain}"
    );
    assert!(
        plain.get("dedup").is_none(),
        "no top-level dedup without the new params: {plain}"
    );
    assert!(
        plain["explanation"].get("dedup").is_none(),
        "inert dedup report omitted from the trace: {plain}"
    );

    let scoped = call(
        &s,
        "compose_context",
        json!({
            "bank": "bank-a",
            "query": "alpha",
            "budget_tokens": 256,
            "recall_scope": "consolidated_only",
            "explain": true
        }),
    );
    assert_eq!(scoped["ok"], json!(true), "{scoped}");
    assert_eq!(
        scoped["explanation"]["dedup"]["scope_applied"],
        json!("consolidated_only"),
        "{scoped}"
    );
}

#[test]
fn scope_params_are_published_in_the_versioned_schema_pack() {
    let pack = schema_pack();
    assert_eq!(pack["mcp_protocol_revision"], json!("2025-11-25"));
    let tool = |name: &str| {
        pack["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|t| t["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name} published"))["inputSchema"]["properties"]
            .clone()
    };
    for name in ["retrieve", "compose_context"] {
        let props = tool(name);
        assert_eq!(
            props["recall_scope"]["enum"],
            json!(["full", "consolidated_only"]),
            "{name} recall_scope published"
        );
        assert_eq!(props["prefer_consolidated"]["type"], json!("boolean"));
    }
}

#[test]
fn compose_scope_keeps_the_persona_partition_and_budget() {
    let s = state();
    duplicate_bank(&s, "bank-a");
    let persona: Vec<Value> = (0..4)
        .map(|i| json!({"key": format!("k{i}"), "text": "terse answers preferred", "score": 0.5}))
        .collect();
    let out = call(
        &s,
        "compose_context",
        json!({
            "bank": "bank-a",
            "query": "alpha",
            "budget_tokens": 256,
            "persona_entries": persona,
            "recall_scope": "consolidated_only"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out["total_tokens"].as_u64().expect("tokens") <= 256);
    let sections = out["sections"].as_array().expect("sections");
    assert_eq!(
        sections[0]["name"],
        json!("persona"),
        "persona section first"
    );
    assert_eq!(sections[1]["item_ids"], json!(["P"]));
    assert_eq!(out["dedup"]["scope_applied"], json!("consolidated_only"));
}

#[test]
fn protocol_handler_matches_direct_dispatch_for_scoped_retrieve() {
    let direct_state = state();
    duplicate_bank(&direct_state, "bank-a");
    let handler_state = state();
    duplicate_bank(&handler_state, "bank-a");
    let args = json!({
        "bank": "bank-a",
        "query": "alpha",
        "recall_scope": "consolidated_only",
        "explain": true
    });
    let direct = call(&direct_state, "retrieve", args.clone());

    let handler = McpHandler::new(handler_state.state_arc());
    let response = handler
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "retrieve", "arguments": args}
        }))
        .expect("handler response");
    assert_eq!(
        strip_volatile(response["result"]["structuredContent"].clone()),
        strip_volatile(direct),
        "protocol layer is the shared seam for both transports"
    );
}
