// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Mutator/record dispatcher tests: update routing, invalidate, persona paths,
//! task/failure writes, consolidate trigger.

use super::dispatch_inner;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::json;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: serde_json::Value) -> serde_json::Value {
    let shaped: clio_types::ToolResult<serde_json::Value> = match dispatch_inner(state, name, &args)
    {
        Ok(data) => clio_types::ToolResult::success(data),
        Err(err) => clio_types::ToolResult::failure(err),
    };
    serde_json::to_value(shaped).expect("envelope")
}

fn stored_item_id(state: &McpState) {
    let _ = crate::write_tools::dispatch(
        state,
        "store",
        &json!({
            "item": {"id": "itm-u1", "subject_id": "subj-u1", "gist": "g"},
            "category": "tool_config",
            "epistemic_kind": "fact",
            "bank": "bank-a"
        }),
    );
}

#[test]
fn update_rejects_fact_rule() {
    let s = state();
    let out = call(
        &s,
        "update",
        json!({
            "target": "itm-1", "new_value": "1", "update_rule": "fact", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("wrong_update_rule"));
}

#[test]
fn update_discrete_invalidates_stored_item() {
    let s = state();
    stored_item_id(&s);
    let out = call(
        &s,
        "update",
        json!({
            "target": "itm-u1", "new_value": "ignored", "update_rule": "discrete", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["rule"], json!("discrete"));
}

#[test]
fn invalidate_closes_both_axes() {
    let s = state();
    stored_item_id(&s);
    let out = call(
        &s,
        "invalidate",
        json!({"item_id": "itm-u1", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let closed = out["closed_ids"].as_array().expect("closed");
    assert!(!closed.is_empty(), "{closed:?}");
}

#[test]
fn persona_paths_split_categorical_vs_scalar() {
    let s = state();
    let put = call(
        &s,
        "persona_put_stable",
        json!({
            "key": "communication_style", "value": "terse", "bank": "bank-a"
        }),
    );
    assert_eq!(put["ok"], json!(true), "{put}");
    assert!(matches!(
        put["outcome"].as_str(),
        Some("admitted" | "rejected")
    ));
    // Scalar payload must be refused on the stable path.
    let scalar = call(
        &s,
        "persona_put_stable",
        json!({
            "key": "verbosity", "value": "0.9", "bank": "bank-a"
        }),
    );
    assert_eq!(scalar["ok"], json!(false));
    assert_eq!(scalar["code"], json!("wrong_update_rule"));
    let observe = call(
        &s,
        "persona_observe_preference",
        json!({
            "key": "verbosity", "observed_value": 0.9, "bank": "bank-a"
        }),
    );
    assert_eq!(observe["ok"], json!(true), "{observe}");
}

#[test]
fn task_upsert_gates_definition() {
    let s = state();
    let out = call(
        &s,
        "task_upsert",
        json!({
            "task_id": "t-1", "definition": "Ship the release", "status": "running",
            "steps": ["build", "test"], "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let missing = call(
        &s,
        "task_upsert",
        json!({
            "task_id": "t-2", "definition": "", "status": "x", "bank": "bank-a"
        }),
    );
    assert_eq!(missing["ok"], json!(false));
}

#[test]
fn failure_record_caps_lesson() {
    let s = state();
    let ok = call(
        &s,
        "failure_record",
        json!({
            "task_id": "t-1", "attempt_n": 1, "what_failed": "test failed",
            "lesson": "check fixtures first", "bank": "bank-a"
        }),
    );
    assert_eq!(ok["ok"], json!(true), "{ok}");
    let long_lesson = "x".repeat(10_000);
    let capped = call(
        &s,
        "failure_record",
        json!({
            "task_id": "t-2", "attempt_n": 1, "what_failed": "f", "lesson": long_lesson,
            "bank": "bank-a"
        }),
    );
    assert_eq!(
        capped["ok"],
        json!(false),
        "over-cap lesson must be rejected"
    );
}

#[test]
fn consolidate_accepts_without_blocking() {
    let s = state();
    let out = call(&s, "consolidate", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out["job_id"].as_str().is_some());
    assert!(matches!(
        out["status"].as_str(),
        Some("accepted" | "completed")
    ));
}

#[test]
fn consolidate_force_refreshes() {
    let s = state();
    let out = call(&s, "consolidate", json!({"bank": "bank-a", "force": true}));
    assert_eq!(out["ok"], json!(true), "{out}");
}

#[test]
fn consolidate_rejects_the_removed_scope_argument() {
    // The selector was renamed to `bank` with no alias: a call that still
    // names `scope` must fail loudly rather than drop the selector and
    // silently target the context bank.
    let s = state();
    let out = call(&s, "consolidate", json!({"scope": "bank-a"}));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
    assert!(
        out["message"].as_str().expect("message").contains("`bank`"),
        "the rejection must name the replacement: {out}"
    );
}

#[test]
fn a_long_lived_binding_keeps_the_async_consolidate_response() {
    let s = state();
    let out = call(&s, "consolidate", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["status"], json!("accepted"), "{out}");
    assert_eq!(out["hubs"], json!("hub_distill:pending"), "{out}");
    assert_eq!(out["refreshed"], json!([]), "{out}");
}

#[test]
fn a_one_shot_binding_completes_the_consolidate_wave() {
    // Hermetic one-shot: an env-free runtime and an empty provider document, so
    // the wave joins on the calling thread.
    let s = McpState::open_with_runtime_effective_and_wave(
        &McpOpenOptions::default(),
        clio_config::Runtime::with_env(&|_| None),
        &serde_json::json!({}),
        true,
    )
    .expect("one-shot state");
    let out = call(&s, "consolidate", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["status"], json!("completed"), "{out}");
    assert!(
        out["hubs"]
            .as_str()
            .unwrap_or_default()
            .starts_with("hub_distill:"),
        "a joined wave must report the real hub summary, got {out}"
    );
}

#[test]
fn update_continuous_routes_to_ema_engine() {
    let s = state();
    // Register the attribute through the persona create gate, then update it.
    let _ = call(
        &s,
        "persona_observe_preference",
        json!({
            "key": "verbosity", "observed_value": 0.5, "bank": "bank-a"
        }),
    );
    let out = call(
        &s,
        "update",
        json!({
            "target": "verbosity", "new_value": 0.8, "update_rule": "continuous", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["rule"], json!("continuous"));
    assert!(out["state"]["state"].as_f64().is_some());
}

#[test]
fn persona_stable_nonexistent_bank_and_empty_args_rejected() {
    let s = state();
    let out = call(
        &s,
        "persona_put_stable",
        json!({"key": "", "value": "v", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(false));
    let obs = call(
        &s,
        "persona_observe_preference",
        json!({"key": "k", "observed_value": "x", "bank": "bank-a"}),
    );
    assert_eq!(obs["ok"], json!(false));
}

#[test]
fn failure_record_bad_attempt_n_rejected() {
    let s = state();
    let out = call(
        &s,
        "failure_record",
        json!({
            "task_id": "t", "attempt_n": 0, "what_failed": "f", "lesson": "l", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("out_of_range"));
}

#[test]
fn update_and_failure_record_missing_required_args() {
    let s = state();
    let out = call(
        &s,
        "update",
        json!({
            "target": "itm-1", "update_rule": "discrete", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));
    let out = call(
        &s,
        "failure_record",
        json!({
            "task_id": "t", "what_failed": "f", "lesson": "l", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));
}

#[test]
fn invalidate_with_an_unknown_replacement_is_refused_and_leaves_the_item_open() {
    use crate::runtime::{McpOpenOptions, McpState};
    use crate::write_tools::dispatch;
    use serde_json::json;
    let s = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    let stored = dispatch(
        &s,
        "store",
        &json!({
            "item": {
                "id": "itm-inv-1", "subject_id": "itm-inv-1", "gist": "terse invalidate answers",
                "snapshot": {"entity": "terse invalidate answers", "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona", "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent",
            "source_text": "The user prefers terse invalidate answers (1 note since 2024-01-01)."
        }),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");
    let refused = dispatch(
        &s,
        "invalidate",
        &json!({"item_id": "itm-inv-1", "replacement_id": "itm-nope", "bank": "bank-a", "actor": "agent"}),
    );
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert_eq!(refused["code"], json!("item_not_found"), "{refused}");
    let closed = dispatch(
        &s,
        "invalidate",
        &json!({"item_id": "itm-inv-1", "bank": "bank-a", "actor": "agent"}),
    );
    assert_eq!(closed["ok"], json!(true), "{closed}");
    assert_eq!(
        closed["closed_ids"].as_array().map(Vec::len),
        Some(1),
        "{closed}"
    );
}
