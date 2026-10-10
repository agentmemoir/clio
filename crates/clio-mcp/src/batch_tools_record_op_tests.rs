// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the `batch` tool's
//! record- and triple-shaped operations: task/persona staging, the triple
//! pre-flight decoder, and failure-record staging.
//!
//! ## Owns
//! - `batch` assertions that stage task, persona, triple, and failure ops
//!
//! ## Does not own
//! - The batch engine and its shared fixtures (see `batch_tools` and
//!   `batch_tools_test_support_tests`)
//! - Store/canonical op assertions (see `batch_tools_tests` and
//!   `batch_tools_canonical_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::batch_tools_test_support_tests::{batch, state};
use serde_json::{Value, json};

#[test]
fn task_and_persona_ops_stage_through_batch() {
    let s = state();
    let ops = json!([
        {"tool": "task_upsert", "params": {
            "task_id": "t-1", "definition": "fix the build", "steps": ["edit", "test"],
            "status": "in_progress", "bank": "bank-a", "actor": "agent"
        }},
        {"tool": "persona_put_stable", "params": {
            "key": "communication_style", "value": "concise", "bank": "bank-a",
            "actor": "agent", "confidence": 0.8
        }}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["operations"][0]["status"], "COMMITTED");
    assert_eq!(out["operations"][1]["status"], "COMMITTED");
    let tasks = s.history.list_tasks("bank-a", 10).unwrap();
    assert_eq!(tasks.len(), 1);
    let doc = s.persona.get_persona_document("bank-a").unwrap();
    assert!(!doc.stable.is_empty(), "{:?}", doc.stable);
}

#[test]
fn triple_add_op_stages_through_batch() {
    let s = state();
    // A prior semantic item is needed for a snapshot_ref-free triple? Not
    // required: triples are episodic. Commit one via batch.
    let ops = json!([
        {"tool": "triple_add", "params": {
            "subject": "alice", "predicate": "lives_in", "object": "boston",
            "epistemic_kind": "fact", "confidence": 0.9,
            "bank": "bank-a", "actor": "agent"
        }}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["operations"][0]["status"], "COMMITTED");
    let triples = s
        .store
        .query_triples(&clio_types::TripleQuery {
            bank_id: "bank-a".into(),
            subject: Some("alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: None,
            time_axis: clio_types::TimeAxis::Valid,
        })
        .unwrap();
    assert_eq!(triples.len(), 1);
}

#[test]
fn failure_record_op_stages_through_batch() {
    let s = state();
    let ops = json!([
        {"tool": "failure_record", "params": {
            "task_id": "t-2", "attempt_n": 1, "what_failed": "compile error",
            "lesson": "run cargo check first", "bank": "bank-a", "actor": "agent"
        }}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");
    let failures = s.history.list_failures("bank-a", 10).unwrap();
    assert_eq!(failures.len(), 1);
}

#[test]
fn batch_triple_context_uses_the_fail_closed_decoder() {
    // The batch triple pre-flight used the permissive `opt_str` decoder,
    // so a present non-string (or explicitly empty) context was silently treated
    // as absent and the operation reported WOULD_COMMIT. It now uses the shared
    // decoder: a malformed context fails the whole batch, a bounded string
    // stages normally.
    let s = state();
    let triple = |context: Value| {
        json!([{
            "tool": "triple_add",
            "params": {
                "subject": "alice", "predicate": "lives_in", "object": "boston",
                "epistemic_kind": "fact", "context": context,
                "bank": "bank-a", "actor": "agent"
            }
        }])
    };
    let non_string = batch(&s, triple(json!(123)), true);
    assert_eq!(non_string["ok"], json!(false), "{non_string}");
    assert!(
        non_string["message"]
            .as_str()
            .unwrap_or("")
            .contains("context must be a string"),
        "{non_string}"
    );
    let empty = batch(&s, triple(json!("")), true);
    assert_eq!(empty["ok"], json!(false), "{empty}");
    assert!(
        empty["message"].as_str().unwrap_or("").contains("context"),
        "{empty}"
    );
    let oversized = batch(&s, triple(json!("x".repeat(4097))), true);
    assert_eq!(oversized["ok"], json!(false), "{oversized}");
    assert!(
        oversized["message"].as_str().unwrap_or("").contains("4096"),
        "{oversized}"
    );
    let valid = batch(&s, triple(json!("bounded note")), true);
    assert_eq!(valid["ok"], json!(true), "{valid}");
    assert_eq!(valid["operations"][0]["status"], "WOULD_COMMIT");
}
