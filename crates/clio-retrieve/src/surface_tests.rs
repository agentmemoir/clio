// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for in-process JSON tool-surface tests: argument
//! parsing, explicit-vs-automatic gate wiring, the bank read-policy
//! resolution, and the belief/fact serialization contract on tool results.
//!
//! ## Owns
//! - Tool-contract assertions for `intent_gate`, `retrieve`, and
//!   `compose_context` JSON arguments and their error results, including the
//!   `min_scores` field-name rejection the bindings share
//! - The `ReadPolicyResolver` resolution for the scope, tolerance, curve state,
//!   provenance floor, and pack fidelity channel, plus the builtin default
//!   profile without a resolver
//! - The `include_exact` toggle's payload effect: a hit carries its bounded
//!   rendering only when the request asked for it
//!
//! ## Does not own
//! - The pack fidelity channel proven against a profile naming `exact` and the
//!   channel's refusal wording (`surface_policy_tests`); the provenance floor's
//!   wire resolution (`provenance_surface_tests`)
//! - Gate/retrieval/compose behavior (`intent_tests`, `hybrid_tests`,
//!   `compose_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::compose::Composer;
use crate::fixtures_tests as fx;
use crate::surface::{
    ReadPolicyResolver, compose_from_json, compose_from_json_with_policy, intent_gate_from_json,
    retrieve_from_json, retrieve_from_json_with_policy,
};
use crate::surface_policy::resolve_policy;
use clio_config::retention::{DuplicateTolerance, RecallScope, RetentionProfile};
use clio_config::retention_levels::PackFidelityDefault;
use serde_json::json;
use std::sync::Arc;

fn retriever() -> (Arc<clio_store::SqliteStore>, crate::hybrid::HybridRetriever) {
    let store = fx::store();
    let retriever = fx::retriever(&store);
    (store, retriever)
}

#[test]
fn intent_gate_tool_happy_and_bad_args() {
    let ok = intent_gate_from_json(&json!({"turn_text": "hi"}));
    let value = serde_json::to_value(&ok).expect("serialize");
    assert_eq!(value["ok"], true);
    assert_eq!(value["retrieval_needed"], false);

    let ok =
        intent_gate_from_json(&json!({"turn_text": "what is my name", "skip_confidence": 0.1}));
    let value = serde_json::to_value(&ok).expect("serialize");
    assert_eq!(value["retrieval_needed"], true);

    let bad = intent_gate_from_json(&json!({"turn_text": 5}));
    let value = serde_json::to_value(&bad).expect("serialize");
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "invalid_argument");
}

#[test]
fn retrieve_tool_happy_path() {
    let (store, retriever) = retriever();
    let item = fx::item("A", "bank", "city live");
    fx::put_lexical(&store, &item, "city live");
    let result = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "city", "limit": 3}),
    );
    let value = serde_json::to_value(&result).expect("serialize");
    assert_eq!(value["ok"], true);
    assert_eq!(value["hits"][0]["item_id"], "A");
    assert_eq!(value["hits"][0]["epistemic_kind"], "fact");
}

/// The in-process surface deserializes `min_scores` through the same type the
/// MCP binding uses, so it rejects a mistyped field name too instead of running
/// the query unfiltered. Both paths must fail closed the same way.
#[test]
fn retrieve_tool_rejects_a_mistyped_min_scores_field_name() {
    let (store, retriever) = retriever();
    let item = fx::item("A", "bank", "city live");
    fx::put_lexical(&store, &item, "city live");

    let typo = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "city", "min_scores": {"final_typo": 0.5}}),
    );
    let value = serde_json::to_value(&typo).expect("serialize");
    assert_eq!(value["ok"], false, "a mistyped floor is never accepted");
    assert_eq!(value["code"], "invalid_argument", "{value}");

    // The documented name still works, so the rejection is about the name. A
    // 0.0 `final` floor is the inclusive no-op that keeps the whole page.
    let ok = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "city", "min_scores": {"final": 0.0}}),
    );
    let value = serde_json::to_value(&ok).expect("serialize");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["hits"][0]["item_id"], "A", "{value}");
}

#[test]
fn retrieve_tool_rejects_bad_axis_domains_and_bank() {
    let (_store, retriever) = retriever();
    let bad_axis = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "q", "time_axis": "sideways"}),
    );
    let value = serde_json::to_value(&bad_axis).expect("serialize");
    assert_eq!(value["code"], "invalid_argument");

    let bad_domain = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "q", "domains": ["bogus"]}),
    );
    let value = serde_json::to_value(&bad_domain).expect("serialize");
    assert_eq!(value["ok"], false);

    let missing_bank = retrieve_from_json(&retriever, &json!({"query": "q"}));
    let value = serde_json::to_value(&missing_bank).expect("serialize");
    assert_eq!(value["ok"], false);

    // An unknown scope value is rejected on the in-process surface too.
    let bad_scope = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "q", "recall_scope": "observations"}),
    );
    let value = serde_json::to_value(&bad_scope).expect("serialize");
    assert_eq!(value["code"], "invalid_argument");
}

#[test]
fn retrieve_tool_defaults_axis_and_limit() {
    let (store, retriever) = retriever();
    let item = fx::item("A", "bank", "city live");
    fx::put_lexical(&store, &item, "city live");
    let result = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "city", "time_axis": "transaction"}),
    );
    let value = serde_json::to_value(&result).expect("serialize");
    assert_eq!(value["ok"], true);
    let empty_axis = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "city", "time_axis": ""}),
    );
    assert_eq!(serde_json::to_value(&empty_axis).expect("ser")["ok"], true);
}

/// The in-process surface can ask for a rendering per hit, and a call that asks
/// for nothing leaves the field absent, so the default page is unchanged.
#[test]
fn retrieve_tool_attaches_the_rendering_only_when_include_exact_is_asked_for() {
    let (store, retriever) = retriever();
    let item = fx::item("A", "bank", "city live");
    fx::put_lexical(&store, &item, "city live");

    let asked = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "city", "include_exact": true}),
    );
    let value = serde_json::to_value(&asked).expect("serialize");
    assert_eq!(
        value["hits"][0]["exact_render"], r#"{"key":"value"}"#,
        "{value}"
    );

    let unnamed = retrieve_from_json(&retriever, &json!({"bank": "bank", "query": "city"}));
    let value = serde_json::to_value(&unnamed).expect("serialize");
    assert!(value["hits"][0].get("exact_render").is_none(), "{value}");
}

#[test]
fn belief_and_fact_fields_serialize_per_contract() {
    let store = fx::store();
    // Distinct texts: a shared one would make these exact duplicates, and
    // read-time collapse would drop the belief before this contract check.
    let belief = fx::belief_item("BEL", "bank", "shared token belief");
    let fact = fx::item("FACT", "bank", "shared token fact");
    fx::put_lexical(&store, &belief, "shared token belief");
    fx::put_lexical(&store, &fact, "shared token fact");
    let retriever = fx::retriever(&store);
    let result = retrieve_from_json(&retriever, &json!({"bank": "bank", "query": "shared"}));
    let value = serde_json::to_value(&result).expect("serialize");
    let hits = value["hits"].as_array().expect("hits");
    let belief_hit = hits
        .iter()
        .find(|h| h["item_id"] == "BEL")
        .expect("belief hit");
    assert_eq!(belief_hit["epistemic_kind"], "belief");
    assert_eq!(belief_hit["source_type"], "agent_inferred");
    assert_eq!(belief_hit["confidence"], 0.42);
    assert_eq!(belief_hit["requires_confidence_check"], true);

    let fact_hit = hits
        .iter()
        .find(|h| h["item_id"] == "FACT")
        .expect("fact hit");
    assert_eq!(fact_hit["epistemic_kind"], "fact");
    assert!(fact_hit.get("source_type").is_none());
    assert!(fact_hit.get("confidence").is_none());
}

#[test]
fn retrieve_tool_surfaces_context_and_omits_key_when_absent() {
    let store = fx::store();
    let mut with_context = fx::item("CTX", "bank", "shared token ctx");
    with_context.context = Some("from a team chat".into());
    let without_context = fx::item("NOCTX", "bank", "shared token plain");
    fx::put_lexical(&store, &with_context, "shared token ctx");
    fx::put_lexical(&store, &without_context, "shared token plain");
    let retriever = fx::retriever(&store);
    let result = retrieve_from_json(&retriever, &json!({"bank": "bank", "query": "shared"}));
    let value = serde_json::to_value(&result).expect("serialize");
    let hits = value["hits"].as_array().expect("hits");

    // An item stored WITH context surfaces it on the JSON hit.
    let ctx_hit = hits
        .iter()
        .find(|h| h["item_id"] == "CTX")
        .expect("ctx hit");
    assert_eq!(ctx_hit["context"], "from a team chat");

    // An item stored WITHOUT context omits the key entirely (no sentinel).
    let plain_hit = hits
        .iter()
        .find(|h| h["item_id"] == "NOCTX")
        .expect("plain hit");
    assert!(plain_hit.get("context").is_none(), "{plain_hit}");
}

#[test]
fn compose_tool_explicit_retrieves_memory() {
    let store = fx::store();
    let item = fx::item("A", "bank", "city live");
    fx::put_lexical(&store, &item, "city live");
    let composer = Composer::new(Arc::new(fx::retriever(&store)));
    let result = compose_from_json(
        &composer,
        &json!({
            "bank": "bank",
            "query": "city",
            "budget_tokens": 400,
            "persona_entries": [{"key": "name", "text": "Ada", "score": 0.9}]
        }),
        None,
    );
    let value = serde_json::to_value(&result).expect("serialize");
    assert_eq!(value["ok"], true);
    assert_eq!(value["retrieval_ran"], true);
    assert_eq!(value["sections"][0]["name"], "persona");
    assert_eq!(value["sections"][1]["item_ids"][0], "A");
}

#[test]
fn compose_tool_auto_path_honors_skip() {
    let store = fx::store();
    let item = fx::item("A", "bank", "city live");
    fx::put_lexical(&store, &item, "city live");
    let composer = Composer::new(Arc::new(fx::retriever(&store)));
    let skipped = compose_from_json(
        &composer,
        &json!({
            "bank": "bank",
            "query": "city",
            "budget_tokens": 400,
            "auto": true,
            "retrieval_needed": false,
            "persona_entries": [{"key": "name", "text": "Ada"}]
        }),
        None,
    );
    let value = serde_json::to_value(&skipped).expect("serialize");
    assert_eq!(value["retrieval_ran"], false);
    let skipped_ids = value["sections"][1]["item_ids"].as_array().expect("ids");
    assert!(skipped_ids.is_empty(), "{skipped_ids:?}");

    let retrieved = compose_from_json(
        &composer,
        &json!({
            "bank": "bank",
            "query": "city",
            "budget_tokens": 400,
            "auto": true,
            "retrieval_needed": true
        }),
        None,
    );
    let value = serde_json::to_value(&retrieved).expect("serialize");
    assert_eq!(value["retrieval_ran"], true);
}

#[test]
fn compose_tool_defaults_budget_and_rejects_missing_bank() {
    let store = fx::store();
    let composer = Composer::new(Arc::new(fx::retriever(&store)));
    let ok = compose_from_json(&composer, &json!({"query": "x"}), None);
    let value = serde_json::to_value(&ok).expect("serialize");
    assert_eq!(value["code"], "missing_bank");

    let ok = compose_from_json(&composer, &json!({"bank": "bank"}), None);
    let value = serde_json::to_value(&ok).expect("serialize");
    assert_eq!(value["ok"], true);
    assert_eq!(value["budget_tokens"], 2048);
}

#[test]
fn compose_tool_bad_args_rejected() {
    let store = fx::store();
    let composer = Composer::new(Arc::new(fx::retriever(&store)));
    let bad = compose_from_json(&composer, &json!({"bank": 5}), None);
    let value = serde_json::to_value(&bad).expect("serialize");
    assert_eq!(value["ok"], false);
}

/// Binding-side bank policy stand-in: `consolidated_only` scope, `strict` cap,
/// curve off — a profile distinct from the builtin default in the scope,
/// tolerance, and curve fields, so each resolution below pins which source
/// supplied it. Its pack fidelity channel stays at the builtin default, which is
/// why the channel is proven in `surface_policy_tests` instead.
struct ConsolidatedBank;

impl ReadPolicyResolver for ConsolidatedBank {
    fn read_policy(&self, _bank: &str) -> RetentionProfile {
        RetentionProfile {
            recall_scope_default: RecallScope::ConsolidatedOnly,
            duplicate_tolerance_read: DuplicateTolerance::Strict,
            retention_curve: false,
            ..RetentionProfile::default()
        }
    }
}

/// The resolver's profile supplies the curve state alongside the scope and
/// tolerance, and the no-resolver surface applies the builtin default profile,
/// provenance floor included. The floor a resolver states is proven where a
/// resolver states one (`provenance_surface_tests`).
#[test]
fn read_policy_resolution_supplies_the_curve_state_and_the_builtin_default() {
    let (scope, tolerance, curve, floor, _, fidelity) =
        resolve_policy(None, None, None, "bank", Some(&ConsolidatedBank)).expect("resolve");
    assert_eq!(scope, RecallScope::ConsolidatedOnly);
    assert_eq!(tolerance, DuplicateTolerance::Strict);
    assert!(!curve, "the resolver's curve flag wins over the default");
    assert_eq!(floor, RetentionProfile::default().retention_floor);
    assert_eq!(fidelity, PackFidelityDefault::Flow);

    let builtin = RetentionProfile::default();
    let (scope, tolerance, curve, floor, provenance, fidelity) =
        resolve_policy(None, None, None, "bank", None).expect("resolve");
    assert_eq!(scope, builtin.recall_scope_default);
    assert_eq!(tolerance, builtin.duplicate_tolerance_read);
    assert_eq!(curve, builtin.retention_curve);
    assert_eq!(floor, builtin.retention_floor);
    assert_eq!(provenance, builtin.provenance_floor_default);
    assert_eq!(fidelity, builtin.pack_fidelity_default);

    // An explicit call parameter still wins over the profile default.
    let (scope, .., fidelity) = resolve_policy(
        Some("full"),
        None,
        Some("exact"),
        "bank",
        Some(&ConsolidatedBank),
    )
    .expect("resolve");
    assert_eq!(scope, RecallScope::Full);
    assert_eq!(fidelity, PackFidelityDefault::Exact);
}

/// One hub-distilled parent claiming one raw item, both lexically queryable.
fn consolidated_fixture() -> (Arc<clio_store::SqliteStore>, crate::hybrid::HybridRetriever) {
    let store = fx::store();
    let mut parent = fx::semantic_item("P", "bank", "shared token parent");
    parent.source_ref = Some(crate::HUB_DISTILL_SOURCE_REF.into());
    parent.snapshot = Some(json!({"hub_id": "HUB", "neighbor_ids": ["A"]}));
    fx::put_lexical(&store, &parent, "shared token parent");
    let raw = fx::item("A", "bank", "shared token raw");
    fx::put_lexical(&store, &raw, "shared token raw");
    let retriever = fx::retriever(&store);
    (store, retriever)
}

#[test]
fn retrieve_json_surface_applies_the_bank_read_policy_when_a_resolver_is_supplied() {
    let (_store, retriever) = consolidated_fixture();
    // Explicit-parameter-only entry point: no resolver, no bank default.
    let plain = retrieve_from_json(&retriever, &json!({"bank": "bank", "query": "shared"}));
    let value = serde_json::to_value(&plain).expect("serialize");
    assert_eq!(value["hits"].as_array().expect("hits").len(), 2);
    assert!(value.get("dedup").is_none(), "{value}");

    // The binding's resolver supplies the bank default scope.
    let scoped = retrieve_from_json_with_policy(
        &retriever,
        &json!({"bank": "bank", "query": "shared"}),
        Some(&ConsolidatedBank),
    );
    let value = serde_json::to_value(&scoped).expect("serialize");
    assert_eq!(value["hits"][0]["item_id"], "P");
    assert_eq!(value["hits"].as_array().expect("hits").len(), 1);
    assert_eq!(value["dedup"]["scope_applied"], "consolidated_only");

    // An explicit call parameter always wins over the bank default.
    let explicit = retrieve_from_json_with_policy(
        &retriever,
        &json!({"bank": "bank", "query": "shared", "recall_scope": "full"}),
        Some(&ConsolidatedBank),
    );
    let value = serde_json::to_value(&explicit).expect("serialize");
    assert_eq!(value["hits"].as_array().expect("hits").len(), 2);
}

#[test]
fn compose_json_surface_applies_the_bank_read_policy_when_a_resolver_is_supplied() {
    let (_store, retriever) = consolidated_fixture();
    let composer = Composer::new(Arc::new(retriever));
    let scoped = compose_from_json_with_policy(
        &composer,
        &json!({"bank": "bank", "query": "shared", "budget_tokens": 400}),
        None,
        Some(&ConsolidatedBank),
    );
    let value = serde_json::to_value(&scoped).expect("serialize");
    assert_eq!(value["sections"][1]["item_ids"], json!(["P"]));
    assert_eq!(value["dedup"]["scope_applied"], "consolidated_only");

    let plain = compose_from_json(
        &composer,
        &json!({"bank": "bank", "query": "shared", "budget_tokens": 400}),
        None,
    );
    let value = serde_json::to_value(&plain).expect("serialize");
    assert_eq!(
        value["sections"][1]["item_ids"]
            .as_array()
            .expect("ids")
            .len(),
        2
    );
    assert!(value.get("dedup").is_none(), "{value}");
}
