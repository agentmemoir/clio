// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP read-surface retention-curve tests:
//! the bank profile's active flag and floor reaching `retrieve` and
//! `compose_context`, the builtin default for a bank with no stored profile,
//! and a bank floor change reflected by the next read.
//!
//! ## Owns
//! - The fixed-clock curve ordering assertions over the `retrieve` payload
//!   (the clock is fixed through the handlers' `now` seam, never a call
//!   argument: the published schemas declare no timestamp)
//! - The `compose_context` memory-section curve assertion
//! - The MCP payload and the policy-resolved in-process surface agreeing for
//!   the same stored bank profile
//!
//! ## Does not own
//! - Curve math (`clio-types` decay and `clio-retrieve` shaping)
//! - Profile storage/resolution (see `runtime_retention`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the MCP binding of the curve policy.

use crate::runtime::{McpOpenOptions, McpState};
use crate::runtime_retention::retention_test_support::RetentionStoreFixture;
use crate::write_tools::{dispatch, tool_result};
use clio_retrieve::retrieve_from_json_with_policy;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};

/// Fixed clock for every call in this module, injected through the handlers'
/// `now` seam — the engine's fixed-clock input. The published `retrieve` and
/// `compose_context` schemas declare no timestamp argument, so a call argument
/// could not fix the clock.
const NOW: &str = "2030-01-01T00:00:00Z";

/// Hermetic state whose retention store is a file the fixture owns, so a profile
/// written here never reaches the store the host environment names.
fn state() -> RetentionStoreFixture {
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

fn base_item(id: &str, bank: &str, gist: &str, created_at: &str) -> MemoryItem {
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
        created_at: created_at.into(),
        updated_at: created_at.into(),
        context: None,
    }
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

/// One old, strong lexical hit and one new, weak one. At `NOW` the old item is
/// far past the half-life, so a low floor drops it while a floor of 1.0 keeps
/// the base order.
fn curve_bank(state: &McpState) {
    put(
        state,
        &base_item(
            "old",
            "bank-a",
            "alpha bravo charlie",
            "2024-01-01T00:00:00Z",
        ),
        "alpha bravo charlie",
    );
    put(
        state,
        &base_item("new", "bank-a", "alpha delta echo", NOW),
        "alpha delta echo",
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

fn hit_by_id<'a>(payload: &'a Value, id: &str) -> &'a Value {
    payload["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .find(|h| h["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))
}

fn set_profile(state: &McpState, profile: &Value) -> Value {
    call(
        state,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": profile, "confirm": true}),
    )
}

fn retrieve(state: &McpState) -> Value {
    tool_result(super::retrieve_at(
        state,
        &json!({"bank": "bank-a", "query": "alpha bravo charlie", "limit": 5}),
        Some(NOW.to_owned()),
    ))
}

fn compose(state: &McpState) -> Value {
    tool_result(super::compose_context_at(
        state,
        &json!({"bank": "bank-a", "query": "alpha bravo charlie", "budget_tokens": 256}),
        Some(NOW.to_owned()),
    ))
}

#[test]
fn bank_floor_change_is_reflected_by_the_next_read() {
    let s = state();
    curve_bank(&s);

    let held = set_profile(
        &s,
        &json!({"retention_curve": true, "retention_floor": 1.0}),
    );
    assert_eq!(held["ok"], json!(true), "{held}");
    let before = retrieve(&s);
    assert_eq!(before["ok"], json!(true), "{before}");
    assert_eq!(hit_ids(&before), vec!["old", "new"], "floor 1.0: {before}");

    let dropped = set_profile(
        &s,
        &json!({"retention_curve": true, "retention_floor": 0.25}),
    );
    assert_eq!(dropped["ok"], json!(true), "{dropped}");
    let after = retrieve(&s);
    assert_eq!(after["ok"], json!(true), "{after}");
    assert_eq!(
        hit_ids(&after),
        vec!["new", "old"],
        "floor 0.25 at the fixed clock: {after}"
    );
}

#[test]
fn bank_without_a_stored_profile_uses_the_builtin_curve_defaults() {
    let s = state();
    curve_bank(&s);

    let out = retrieve(&s);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        hit_ids(&out),
        vec!["new", "old"],
        "builtin curve on, floor 0.25: {out}"
    );

    let profile = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(profile["source"], json!("builtin"), "{profile}");
    assert_eq!(profile["retention_curve"], json!(true), "{profile}");
    assert_eq!(profile["retention_floor"], json!(0.25), "{profile}");
    assert_eq!(profile["pack_fidelity_default"], json!("flow"), "{profile}");
    assert_eq!(
        profile["provenance_floor_default"],
        json!(null),
        "{profile}"
    );
    assert_eq!(profile["maintenance_trigger"], json!(true), "{profile}");
}

#[test]
fn stored_curve_off_restores_the_pre_curve_order() {
    let s = state();
    curve_bank(&s);

    let off = set_profile(
        &s,
        &json!({"retention_curve": false, "retention_floor": 0.25}),
    );
    assert_eq!(off["ok"], json!(true), "{off}");
    let out = retrieve(&s);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        hit_ids(&out),
        vec!["old", "new"],
        "curve off keeps the base order: {out}"
    );
}

#[test]
fn compose_context_memory_section_receives_the_bank_curve() {
    let s = state();
    curve_bank(&s);

    let held = set_profile(
        &s,
        &json!({"retention_curve": true, "retention_floor": 1.0}),
    );
    assert_eq!(held["ok"], json!(true), "{held}");
    let base = compose(&s);
    assert_eq!(base["ok"], json!(true), "{base}");
    let memory = memory_section(&base);
    assert_eq!(
        memory["item_ids"],
        json!(["old", "new"]),
        "floor 1.0: {base}"
    );

    let dropped = set_profile(
        &s,
        &json!({"retention_curve": true, "retention_floor": 0.25}),
    );
    assert_eq!(dropped["ok"], json!(true), "{dropped}");
    let curved = compose(&s);
    assert_eq!(curved["ok"], json!(true), "{curved}");
    let memory = memory_section(&curved);
    assert_eq!(
        memory["item_ids"],
        json!(["new", "old"]),
        "floor 0.25 at the fixed clock: {curved}"
    );
}

#[test]
fn ranking_env_reports_the_retention_half_life_default() {
    let s = state();
    let env = call(&s, "ranking_env_get", json!({}));
    assert_eq!(env["ok"], json!(true), "{env}");
    assert_eq!(
        env["retention"]["half_life_days"],
        json!(30.0),
        "the retention shaping group is published with its default: {env}"
    );
}

/// The MCP payload and the policy-resolved in-process surface agree for the
/// same stored bank profile: the bank override reaches both the handler's
/// `read_curve` and the surface's `ReadPolicyResolver` (one
/// `retention_effective` resolution). `final` is never compared: the MCP call
/// pins its clock through the handler seam, while the in-process request
/// carries no `now`, so the engine reads its own system clock and the curve's
/// smooth age factor differs with the reading. The arm scores are clock-free
/// and are compared.
#[test]
fn mcp_and_policy_resolved_in_process_surfaces_agree_for_the_bank_profile() {
    let s = state();
    curve_bank(&s);

    let held = set_profile(
        &s,
        &json!({"retention_curve": true, "retention_floor": 0.5}),
    );
    assert_eq!(held["ok"], json!(true), "{held}");
    let profile = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(profile["source"], json!("bank"), "{profile}");
    assert_eq!(profile["retention_curve"], json!(true), "{profile}");
    assert_eq!(profile["retention_floor"], json!(0.5), "{profile}");

    let args = json!({"bank": "bank-a", "query": "alpha bravo charlie", "limit": 5});
    let mcp = retrieve(&s);
    assert_eq!(mcp["ok"], json!(true), "{mcp}");
    let in_process = serde_json::to_value(retrieve_from_json_with_policy(
        &s.retriever,
        &args,
        Some(&*s),
    ))
    .expect("serialize in-process outcome");
    assert_eq!(in_process["ok"], json!(true), "{in_process}");

    let mut mcp_ids = hit_ids(&mcp);
    let mut in_process_ids = hit_ids(&in_process);
    mcp_ids.sort_unstable();
    in_process_ids.sort_unstable();
    assert_eq!(
        mcp_ids, in_process_ids,
        "same store, same profile, same hits: {mcp} vs {in_process}"
    );
    assert!(!mcp_ids.is_empty(), "the query matched seeded items: {mcp}");

    for id in &mcp_ids {
        let mcp_hit = hit_by_id(&mcp, id);
        let in_process_hit = hit_by_id(&in_process, id);
        for key in ["semantic", "keyword", "reranker"] {
            assert_eq!(
                mcp_hit["scores"][key], in_process_hit["scores"][key],
                "clock-free `{key}` parity for `{id}`: {mcp_hit} vs {in_process_hit}"
            );
        }
    }
}

fn memory_section(pack: &Value) -> &Value {
    pack["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|s| s["name"] == json!("memory"))
        .unwrap_or_else(|| panic!("memory section present: {pack}"))
}

/// The published read binding honours the state's pinned clock: two different
/// pins report different `scores.age_factor` at this item's age. If the binding
/// stops reading the state clock, both calls fall back to one system reading and
/// the factors are equal, so this test fails.
#[test]
fn the_pinned_state_clock_moves_the_reported_age_factor() {
    let mut earlier = state();
    earlier.set_test_now("2026-09-14T00:00:00Z");
    put(
        &earlier,
        &base_item(
            "old",
            "bank-a",
            "alpha bravo charlie",
            "2024-01-01T00:00:00Z",
        ),
        "alpha bravo charlie",
    );
    let mut later = state();
    later.set_test_now("2026-10-14T00:00:00Z");
    put(
        &later,
        &base_item(
            "old",
            "bank-a",
            "alpha bravo charlie",
            "2024-01-01T00:00:00Z",
        ),
        "alpha bravo charlie",
    );
    let args = json!({"bank": "bank-a", "query": "alpha bravo charlie"});
    let first = call(&earlier, "retrieve", args.clone());
    let second = call(&later, "retrieve", args);
    let first_factor = first["hits"][0]["scores"]["age_factor"]
        .as_f64()
        .expect("age factor");
    let second_factor = second["hits"][0]["scores"]["age_factor"]
        .as_f64()
        .expect("age factor");
    assert_ne!(
        first_factor.to_bits(),
        second_factor.to_bits(),
        "{first} vs {second}"
    );
}
