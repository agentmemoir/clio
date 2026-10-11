// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP binding tests of the transparency and
//! correction surface: `correct` confirm gating, reason validation,
//! `audit_trail` reconstruction, and inspect page caps.
//!
//! ## Owns
//! - Binding-level tests for `correct` / `audit_trail` / inspect caps
//!
//! ## Does not own
//! - In-process correction semantics (`clio-compliance`)
//! - Storage SQL (`clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_types::{AttributeSchema, UpdateRule};
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_one(state: &McpState, bank: &str, id: &str, text: &str) {
    let out = call(
        state,
        "store",
        json!({
            "item": {
                "id": id,
                "subject_id": "subj",
                "gist": text,
                "snapshot": {"entity": text, "amount": 1, "date": "2024-01-01"}
            },
            "category": "task_spec",
            "epistemic_kind": "fact",
            "source_text": format!("The user prefers {text} (1 note since 2024-01-01)."),
            "bank": bank,
            "actor": "agent",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(out["ok"], json!(true), "store {id}: {out}");
}

#[test]
fn correct_dry_run_then_confirm_supersedes() {
    let s = state();
    let bank = "bank-mcp-correct";
    store_one(&s, bank, "itm-mcp-c1", "meeting notes alpha");
    let args = json!({
        "bank": bank,
        "item_id": "itm-mcp-c1",
        "new_value": {"meeting": "3pm"},
        "reason": "wrong meeting time",
        "timestamp": "2024-06-01T00:00:00Z",
        "actor": "user"
    });
    // No confirm: zero-write preview.
    let preview = call(&s, "correct", args.clone());
    assert_eq!(preview["ok"], json!(true), "{preview}");
    assert_eq!(preview["confirm"], json!(false));
    assert_eq!(preview["would_correct"]["item_id"], json!("itm-mcp-c1"));
    // The dry-run validates the mandatory reason too (security acceptance).
    let mut spaced_reason = args.clone();
    spaced_reason["reason"] = json!("   ");
    let rejected = call(&s, "correct", spaced_reason);
    assert_eq!(rejected["ok"], json!(false), "{rejected}");
    assert_eq!(rejected["code"], json!("invalid_argument"));
    // The prior is still open after the preview.
    let check = call(&s, "get", json!({"bank": bank, "item_id": "itm-mcp-c1"}));
    assert!(check["tx_until"].is_null(), "{check}");
    // Confirmed: discrete correction.
    let mut confirmed = args;
    confirmed["confirm"] = json!(true);
    let out = call(&s, "correct", confirmed);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["rule"], json!("discrete"));
    assert_eq!(out["correction"]["prior_item_id"], json!("itm-mcp-c1"));
    let new_id = out["correction"]["new_item_id"]
        .as_str()
        .expect("new id")
        .to_owned();
    // Prior closed, corrected value active.
    let prior = call(&s, "get", json!({"bank": bank, "item_id": "itm-mcp-c1"}));
    assert_eq!(prior["tx_until"], json!("2024-06-01T00:00:00Z"), "{prior}");
    assert_eq!(prior["snapshot"]["entity"], json!("meeting notes alpha"));
    let active = call(&s, "get", json!({"bank": bank, "item_id": new_id}));
    assert_eq!(active["snapshot"]["meeting"], json!("3pm"));
    // audit_trail reconstructs the lifecycle across the lineage.
    let trail = call(
        &s,
        "audit_trail",
        json!({"bank": bank, "item_id": "itm-mcp-c1"}),
    );
    assert_eq!(trail["ok"], json!(true), "{trail}");
    let ops: Vec<&str> = trail["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|e| e["operation"].as_str().unwrap_or(""))
        .collect();
    assert!(ops.contains(&"create_memory_item"), "{ops:?}");
    assert!(ops.contains(&"correct"));
    let correct_event = trail["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|e| e["operation"] == "correct")
        .expect("correct event");
    assert_eq!(correct_event["actor"], json!("user"));
    assert_eq!(correct_event["reason"], json!("wrong meeting time"));
    // Trail from the new revision id reaches the same lineage.
    let trail_new = call(&s, "audit_trail", json!({"bank": bank, "item_id": new_id}));
    assert_eq!(trail_new["lineage_ids"][0], json!("itm-mcp-c1"));
    // Missing reason fails closed at the binding.
    let no_reason = call(
        &s,
        "correct",
        json!({
            "bank": bank,
            "item_id": new_id,
            "new_value": {"meeting": "4pm"},
            "reason": "",
            "confirm": true
        }),
    );
    assert_eq!(no_reason["ok"], json!(false));
    assert_eq!(no_reason["code"], json!("invalid_argument"));
}

#[test]
fn audit_trail_missing_item_and_inspect_page_cap() {
    let s = state();
    let bank = "bank-mcp-audit";
    for i in 0..3 {
        // Distinct text per item: a shared one is an exact duplicate, and the
        // unconditional write-side rule refuses the second store.
        store_one(
            &s,
            bank,
            &format!("itm-{i}"),
            &format!("meeting notes alpha itm-{i}"),
        );
    }
    let missing = call(&s, "audit_trail", json!({"bank": bank, "item_id": "ghost"}));
    assert_eq!(missing["ok"], json!(false));
    assert_eq!(missing["code"], json!("item_not_found"));
    // Page cap: a limit above 200 is clamped to 200 by the binding.
    let oversized = call(&s, "inspect", json!({"bank": bank, "limit": 300}));
    assert_eq!(oversized["ok"], json!(true), "{oversized}");
    assert_eq!(oversized["limit"], json!(200));
    assert_eq!(oversized["total"], json!(3), "{oversized}");
    let paged = call(
        &s,
        "inspect",
        json!({"bank": bank, "limit": 2, "offset": 1}),
    );
    assert_eq!(paged["ok"], json!(true), "{paged}");
    assert_eq!(paged["items"].as_array().expect("items").len(), 2);
    assert_eq!(paged["limit"], json!(2));
    // Total is the full match count, independent of the page window.
    assert_eq!(paged["total"], json!(3), "{paged}");
}

#[test]
fn correct_continuous_attribute_via_binding() {
    let s = state();
    let bank = "bank-mcp-cont";
    // Declare a continuous attribute via the schema tool surface.
    let put = call(&s, "config_get", json!({"bank": bank}));
    let _ = put;
    // Register the schema through the store directly (attribute registration
    // has no MCP tool; the continuous path is exercised in-process).
    s.continuous
        .put_attribute_schema(
            &AttributeSchema {
                bank_id: bank.to_owned(),
                attr_key: "affinity".into(),
                update_rule: UpdateRule::Continuous,
                alpha: Some(0.5),
                trend_window: None,
                bounds_lo: None,
                bounds_hi: None,
                clamp_values: false,
                created_at: "2024-01-01T00:00:00Z".into(),
                updated_at: None,
            },
            "agent",
        )
        .expect("schema");
    let preview = call(
        &s,
        "correct",
        json!({
            "bank": bank,
            "item_id": "affinity",
            "new_value": "0.4",
            "reason": "operator adjusted"
        }),
    );
    assert_eq!(preview["ok"], json!(true), "{preview}");
    let out = call(
        &s,
        "correct",
        json!({
            "bank": bank,
            "item_id": "affinity",
            "new_value": "0.4",
            "reason": "operator adjusted",
            "confirm": true,
            "timestamp": "2024-01-02T00:00:00Z"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["rule"], json!("continuous"));
    assert!((out["state"]["state"].as_f64().unwrap_or(f64::NAN) - 0.4).abs() < 1e-9);
    // Non-numeric payload fails closed.
    let bad = call(
        &s,
        "correct",
        json!({
            "bank": bank,
            "item_id": "affinity",
            "new_value": "not-a-number",
            "reason": "r",
            "confirm": true
        }),
    );
    assert_eq!(bad["ok"], json!(false));
    assert_eq!(bad["code"], json!("invalid_type"));
}

#[allow(clippy::needless_pass_by_value)]
fn call_tool(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Dry-run through the direct bindings must score without persisting (the
/// in-process handlers expose real no-write paths; -adjacent harness rule).
#[test]
fn dry_run_records_score_without_writing() {
    let s = state();
    let persona = call_tool(
        &s,
        "persona_put_stable",
        json!({
            "key": "communication_style", "value": "concise",
            "confidence": 0.8, "bank": "b", "actor": "agent", "dry_run": true
        }),
    );
    assert_eq!(persona["ok"], json!(true), "{persona}");
    assert_eq!(persona["outcome"], "dry_run");
    assert!(persona["entry"]["admission_score"].as_f64().is_some());
    let doc = clio_store::PersonaStore::get_persona_document(&*s.persona, "bank-a").unwrap();
    assert!(doc.stable.is_empty(), "{:?}", doc.stable);

    let task = call_tool(
        &s,
        "task_upsert",
        json!({
            "task_id": "t-dry", "definition": "fix the build",
            "steps": ["edit"], "status": "in_progress",
            "bank": "b", "actor": "agent", "dry_run": true
        }),
    );
    assert_eq!(task["ok"], json!(true), "{task}");
    assert_eq!(task["dry_run"], json!(true));
    assert_eq!(s.history.list_tasks("b", 10).unwrap().len(), 0);

    let failure = call_tool(
        &s,
        "failure_record",
        json!({
            "task_id": "t-dry", "attempt_n": 1, "what_failed": "compile error",
            "lesson": "run cargo check first", "bank": "b", "actor": "agent",
            "dry_run": true
        }),
    );
    assert_eq!(failure["ok"], json!(true), "{failure}");
    assert_eq!(failure["dry_run"], json!(true));
    assert_eq!(s.history.list_failures("b", 10).unwrap().len(), 0);
}

/// `update` accepts numeric `new_value` for the discrete route (stringified).
#[test]
fn update_numeric_new_value_routes_discrete() {
    let s = state();
    let args = json!({
        "item": {"id": "itm-num", "subject_id": "s", "gist": "retry limit 3"},
        "category": "schema", "epistemic_kind": "fact",
        "bank": "b", "actor": "agent"
    });
    let stored = call_tool(&s, "store", args);
    assert_eq!(stored["ok"], json!(true), "{stored}");

    let out = call_tool(
        &s,
        "update",
        json!({
            "target": "itm-num", "new_value": 3, "update_rule": "discrete",
            "bank": "b", "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["rule"], "discrete");
}

/// `correct` without `new_value` fails closed (400-class argument error).
#[test]
fn correct_missing_new_value_errors() {
    let s = state();
    let out = call_tool(
        &s,
        "correct",
        json!({"item_id": "missing", "reason": "why", "bank": "b", "actor": "agent"}),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));
}
