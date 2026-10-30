// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the task readers name an unknown task as not found,
//! that a known task with no failures still reads as an empty list, and that a
//! triple query distinguishes a missing subject from a subject with no
//! matching triples.
//!
//! ## Owns
//! - Assertions over `task_get`, `task_history`, `failures_for_task`, and
//!   `triple_query` for unknown and known targets
//!
//! ## Does not own
//! - The history store's record shapes (see `clio-history`)
//!
//! ## Boundary
//!
//! Changes to how a missing task or a missing triple subject is reported
//! should normally require changes here. Changes to the task record shape
//! should not.
//!
//! Keep this module focused on the task readers' and triple query's not-found
//! rule.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};

fn state() -> std::sync::Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn read(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn upsert_task(state: &McpState, task_id: &str) {
    let out = read(
        state,
        "task_upsert",
        json!({
            "task_id": task_id, "definition": "run the suite", "status": "running",
            "bank": "bank-a", "actor": "agent"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
}

fn seed_triple(state: &McpState, subject: &str, predicate: &str, object: &str) {
    let out = read(
        state,
        "triple_add",
        json!({
            "bank": "bank-a", "subject": subject, "predicate": predicate,
            "object": object, "epistemic_kind": "fact",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
}

#[test]
fn task_get_of_an_unknown_task_is_not_found() {
    let s = state();
    let out = read(&s, "task_get", json!({"task_id": "tsk-nope"}));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_found"), "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or_default()
            .contains("tsk-nope"),
        "{out}"
    );
}

#[test]
fn task_history_of_an_unknown_task_is_not_found() {
    let s = state();
    let out = read(
        &s,
        "task_history",
        json!({"task_id": "tsk-nope", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_found"), "{out}");
}

#[test]
fn failures_for_an_unknown_task_is_not_found() {
    let s = state();
    let out = read(
        &s,
        "failures_for_task",
        json!({"task_id_or_query": "s8nope", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_found"), "{out}");
}

#[test]
fn a_known_task_with_no_failures_lists_none() {
    let s = state();
    upsert_task(&s, "t-clean");
    let out = read(
        &s,
        "failures_for_task",
        json!({"task_id_or_query": "t-clean", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["failures"].as_array().map(Vec::len), Some(0), "{out}");
}

#[test]
fn task_get_reads_a_known_task_back() {
    let s = state();
    upsert_task(&s, "t-known");
    let out = read(
        &s,
        "task_get",
        json!({"task_id": "t-known", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(!out["record"].is_null(), "{out}");
}

#[test]
fn triple_query_of_an_unknown_subject_is_not_found() {
    let s = state();
    seed_triple(&s, "rust", "used_by", "agent");
    let out = read(
        &s,
        "triple_query",
        json!({"subject": "ghost", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_found"), "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ghost"),
        "{out}"
    );
}

#[test]
fn triple_query_of_a_known_subject_with_no_matching_triples_lists_none() {
    let s = state();
    seed_triple(&s, "rust", "used_by", "agent");
    let out = read(
        &s,
        "triple_query",
        json!({"subject": "rust", "predicate": "unknown", "bank": "bank-a"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["triples"].as_array().map(Vec::len), Some(0), "{out}");
}

#[test]
fn triple_query_without_a_subject_lists_the_bank() {
    let s = state();
    seed_triple(&s, "rust", "used_by", "agent");
    let out = read(&s, "triple_query", json!({"bank": "bank-a"}));
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["triples"].as_array().map(Vec::len), Some(1), "{out}");
}
