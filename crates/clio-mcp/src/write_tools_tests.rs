// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Dispatcher tests: gated store pass/reject, dry-run, triple gates, discard
//! confirmation, belief create/append, graph links, unknown tool rejection.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::json;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn good_item_args() -> Value {
    json!({
        "item": {
            "id": "itm-1",
            "subject_id": "subj-1",
            "gist": "prefers terse answers itm-1",
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": "The user prefers terse answers itm-1 (1 note since 2024-01-01).",
        "bank": "bank-a",
        "actor": "agent"
    })
}

/// The same fixture under a different item id.
///
/// The gist carries the id, because two items sharing one gist are exact
/// duplicates and the unconditional write-side rule refuses the second store.
fn good_item_args_for(id: &str) -> Value {
    let mut args = good_item_args();
    args["item"]["id"] = json!(id);
    args["item"]["subject_id"] = json!(format!("subj-{id}"));
    args["item"]["gist"] = json!(format!("prefers terse answers {id}"));
    args["source_text"] = json!(format!(
        "The user prefers terse answers {id} (1 note since 2024-01-01)."
    ));
    args
}

#[test]
fn store_admitted_returns_id_and_score() {
    let s = state();
    let out = call(&s, "store", good_item_args());
    assert_eq!(out["ok"], json!(true), "{out}");
    // Decision fields are top-level, next to the stored id.
    assert!(out["pass"].as_bool().unwrap_or(false));
    assert!(out["admission_score"].as_f64().is_some());
    assert_eq!(out["id"], json!("itm-1"));
    // The leaf is queryable immediately.
    let leaf = s.leaves.get("bank-a", "itm-1");
    assert!(leaf.is_some(), "leaf must publish before ancestor refresh");
}

#[test]
fn store_missing_category_rejected() {
    let s = state();
    let mut args = good_item_args();
    args["item"] = json!({"id": "itm-x"});
    args["source_text"] = json!(Value::Null);
    args["snapshot"] = json!(Value::Null);
    args.as_object_mut().expect("obj").remove("category");
    let out = call(&s, "store", args);
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));
}

#[test]
fn store_dry_run_writes_nothing() {
    let s = state();
    let mut args = good_item_args();
    args["dry_run"] = json!(true);
    let out = call(&s, "store", args);
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["dry_run"], json!(true));
    assert!(
        s.leaves.get("bank-a", "itm-1").is_none(),
        "dry run must not publish"
    );
}

#[test]
fn admit_preview_never_writes() {
    let s = state();
    let mut args = good_item_args();
    args["item"]["id"] = json!("itm-preview");
    let out = call(&s, "admit_preview", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out["pass"].is_boolean());
    assert_eq!(out["dry_run"], json!(true));
    assert!(s.leaves.get("bank-a", "itm-preview").is_none());
}

#[test]
fn span_verify_failure_is_structured_rejection() {
    let s = state();
    let mut args = good_item_args();
    args["item"]["snapshot"] =
        json!({"entity": "Ada Lovelace", "amount": 42, "date": "1852-01-01"});
    args["source_text"] = json!("Completely unrelated text with no matching span.");
    let out = call(&s, "store", args);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(!out["pass"].as_bool().unwrap_or(true));
    assert!(
        out["rejection_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("span_verify")
    );
}

#[test]
fn triple_add_gated_and_triple_end_expires() {
    let s = state();
    let out = call(
        &s,
        "triple_add",
        json!({
            "subject": "rust", "predicate": "used_by", "object": "agent",
            "epistemic_kind": "fact", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(out["new_edge_id"].as_str().is_some());
    let end = call(
        &s,
        "triple_end",
        json!({
            "subject": "rust", "predicate": "creates", "valid_until": "t1", "bank": "bank-a"
        }),
    );
    assert_eq!(end["ok"], json!(true), "{end}");
}

#[test]
fn triple_add_rejects_continuous_update_rule() {
    let s = state();
    let out = call(
        &s,
        "triple_add",
        json!({
            "subject": "a", "predicate": "b", "object": "c",
            "epistemic_kind": "fact", "update_rule": "continuous", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("wrong_update_rule"));
}

#[test]
fn discard_requires_confirm_to_mutate() {
    let s = state();
    let stored = call(&s, "store", good_item_args());
    assert_eq!(stored["ok"], json!(true));
    let dry = call(
        &s,
        "discard",
        json!({"item_id": "itm-1", "reason": "ops cleanup", "bank": "bank-a"}),
    );
    assert_eq!(dry["confirm"], json!(false));
    assert_eq!(dry["would_discard"]["item_id"], json!("itm-1"));
    let confirmed = call(
        &s,
        "discard",
        json!({
            "item_id": "itm-1", "reason": "ops cleanup", "confirm": true, "bank": "bank-a"
        }),
    );
    assert_eq!(confirmed["confirm"], json!(true), "{confirmed}");
    assert_eq!(confirmed["discarded"]["item_id"], json!("itm-1"));
    let again = call(
        &s,
        "discard",
        json!({
            "item_id": "itm-1", "reason": "ops cleanup", "confirm": true, "bank": "bank-a"
        }),
    );
    assert_eq!(again["ok"], json!(false), "repeat discard must be rejected");
}

#[test]
fn belief_observe_create_then_append() {
    let s = state();
    let args = json!({
        "proposition": "Deploys on Fridays are risky",
        "confidence": 0.7,
        "evidence_ref": "ev-1",
        "source_type": "user_stated",
        "bank": "bank-a"
    });
    let create = call(&s, "belief_observe", args.clone());
    assert_eq!(create["ok"], json!(true), "{create}");
    assert_eq!(create["outcome"], json!("created"));
    let append = call(&s, "belief_observe", args);
    assert_eq!(append["ok"], json!(true), "{append}");
    assert_eq!(append["outcome"], json!("appended"));
}

#[test]
fn graph_link_declares_explicit_edge() {
    let s = state();
    let a = call(&s, "store", good_item_args());
    assert_eq!(a["ok"], json!(true), "{a}");
    let b = call(&s, "store", good_item_args_for("itm-2"));
    assert_eq!(b["ok"], json!(true), "{b}");
    let link = call(
        &s,
        "graph_link",
        json!({
            "source_id": "itm-1", "target_id": "itm-2",
            "relationship": "related_to", "bank": "bank-a"
        }),
    );
    assert_eq!(link["ok"], json!(true), "{link}");
}

#[test]
fn graph_link_uses_connection_default_bank() {
    let s = state();
    for id in ["itm-def-1", "itm-def-2"] {
        let mut args = good_item_args_for(id);
        args.as_object_mut().expect("obj").remove("bank");
        let out = call(&s, "store", args);
        assert_eq!(out["ok"], json!(true), "{out}");
    }
    // No `bank` arg: the schema marks it optional, so the connection default
    // (`default`) must be injected rather than failing deserialization.
    let link = call(
        &s,
        "graph_link",
        json!({"source_id": "itm-def-1", "target_id": "itm-def-2", "relationship": "rel"}),
    );
    assert_eq!(link["ok"], json!(true), "{link}");
    assert_eq!(link["source_id"], json!("itm-def-1"));
}

#[test]
fn dry_run_unsupported_mutators_fail_closed() {
    let s = state();
    let out = call(
        &s,
        "triple_add",
        json!({
            "subject": "s", "predicate": "p", "object": "o",
            "epistemic_kind": "fact", "dry_run": true, "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_implemented"));
    // `store` keeps a real dry-run path.
    let mut ok = good_item_args();
    ok["dry_run"] = json!(true);
    assert_eq!(call(&s, "store", ok)["dry_run"], json!(true));
}

#[test]
fn unknown_tool_is_not_implemented() {
    let s = state();
    let out = call(&s, "no_such_tool", json!({}));
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("not_implemented"));
}

#[test]
fn missing_bank_rejected() {
    let s = state();
    let mut args = good_item_args();
    args["bank"] = json!("");
    let out = call(&s, "store", args);
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("missing_bank"));
}

#[test]
fn admit_preview_missing_source_text_errors() {
    let s = state();
    let mut args = good_item_args();
    args["source_text"] = json!(Value::Null);
    let out = call(&s, "admit_preview", args);
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));
}

#[test]
fn triple_end_with_object_filter_and_graph_link_invalid_args() {
    let s = state();
    let add = call(
        &s,
        "triple_add",
        json!({
            "subject": "s1", "predicate": "p1", "object": "o1",
            "epistemic_kind": "fact", "bank": "bank-a"
        }),
    );
    assert_eq!(add["ok"], json!(true), "{add}");
    let end = call(
        &s,
        "triple_end",
        json!({
            "subject": "s1", "predicate": "p1", "object": "o1",
            "valid_until": "t2", "bank": "bank-a"
        }),
    );
    assert_eq!(end["ok"], json!(true), "{end}");
    let bad = call(
        &s,
        "graph_link",
        json!({"source_id": "x", "bank": "bank-a"}),
    );
    assert_eq!(bad["ok"], json!(false));
    let belief = call(
        &s,
        "belief_observe",
        json!({
            "proposition": "p", "evidence_ref": "e", "source_type": "user_stated",
            "bank": "bank-a"
        }),
    );
    assert_eq!(belief["ok"], json!(false), "missing confidence rejected");
}

#[test]
fn triple_add_belief_requires_source_type() {
    let s = state();
    let out = call(
        &s,
        "triple_add",
        json!({
            "subject": "s", "predicate": "p", "object": "o",
            "epistemic_kind": "belief", "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
}

#[test]
fn store_defaults_when_item_shell_and_ids_are_absent() {
    let s = state();
    let out = call(
        &s,
        "store",
        json!({
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "The user prefers terse answers.",
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"},
            "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let id = out["id"].as_str().expect("generated id");
    assert!(id.starts_with("itm-mcp-"), "generated id: {id}");
    // Leaf published for the generated id (no caller subject_id either).
    assert!(s.leaves.get("bank-a", id).is_some());
}
