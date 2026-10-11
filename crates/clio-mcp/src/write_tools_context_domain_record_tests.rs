// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the optional bounded
//! `context` parameter on the domain-record write tools (`persona_put_stable`,
//! `persona_observe_preference`, `task_upsert`, `failure_record`, `triple_add`,
//! `belief_observe`) and its surfacing on the matching domain reads.
//!
//! ## Owns
//! - Write-with-context -> read-surfacing parity assertions per record type
//! - Key-omission assertions for records written without context
//!
//! ## Does not own
//! - Store-level persistence coverage (`clio-store` parity tests)
//! - Item-level `store`/`admit` context coverage (`context_write_tests`)
//! - Schema declaration assertions (`context_read_schema_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::read_history;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn write(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn read(state: &McpState, name: &str, args: &Value) -> Value {
    read_history::dispatch(state, name, args)
        .expect("read tool routed")
        .expect("read succeeds")
}

fn persona_get(state: &McpState) -> Value {
    crate::read_tools::dispatch(state, "persona_get", &json!({ "bank": "bank-a" }))
        .expect("persona_get succeeds")["document"]
        .clone()
}

#[test]
fn persona_put_stable_context_round_trips_through_persona_get() {
    let state = state();
    write(
        &state,
        "persona_put_stable",
        json!({
            "key": "communication_style",
            "value": "concise",
            "source": "explicit",
            "context": "paired debugging sessions",
            "bank": "bank-a"
        }),
    );
    let doc = persona_get(&state);
    let stable = doc["stable"].as_array().expect("stable array");
    let entry = stable
        .iter()
        .find(|e| e["key"] == "communication_style")
        .expect("entry present");
    assert_eq!(entry["context"], "paired debugging sessions");

    // A record written without context omits the key.
    write(
        &state,
        "persona_put_stable",
        json!({ "key": "tone", "value": "plain", "source": "explicit", "bank": "bank-a" }),
    );
    let doc = persona_get(&state);
    let entry = doc["stable"]
        .as_array()
        .expect("stable array")
        .iter()
        .find(|e| e["key"] == "plain" || e["key"] == "tone")
        .cloned()
        .unwrap_or_else(|| {
            doc["stable"]
                .as_array()
                .expect("stable array")
                .last()
                .cloned()
                .expect("last entry")
        });
    assert!(
        !entry.to_string().contains("\"context\""),
        "no-context record must omit the key: {entry}"
    );
}

#[test]
fn task_upsert_failure_record_context_round_trips_and_omits_when_absent() {
    let state = state();
    write(
        &state,
        "task_upsert",
        json!({
            "task_id": "t-1",
            "definition": "Ship the release",
            "status": "in_progress",
            "context": "release week",
            "bank": "bank-a"
        }),
    );
    let got = read(
        &state,
        "task_get",
        &json!({ "task_id": "t-1", "bank": "bank-a" }),
    );
    assert_eq!(got["record"]["context"], "release week");
    let history = read(
        &state,
        "task_history",
        &json!({ "task_id": "t-1", "bank": "bank-a" }),
    );
    let first = history["records"]
        .as_array()
        .expect("records array")
        .first()
        .expect("entry");
    assert_eq!(first["context"], "release week");

    // Without context the key is omitted on the newest version.
    write(
        &state,
        "task_upsert",
        json!({
            "task_id": "t-2",
            "definition": "Write the docs",
            "status": "open",
            "bank": "bank-a"
        }),
    );
    let got = read(
        &state,
        "task_get",
        &json!({ "task_id": "t-2", "bank": "bank-a" }),
    );
    assert!(
        !serde_json::to_string(&got["record"])
            .expect("serialize record")
            .contains("\"context\""),
        "no-context record must omit the key: {got}"
    );

    write(
        &state,
        "failure_record",
        json!({
            "task_id": "t-1",
            "attempt_n": 1,
            "what_failed": "build broke",
            "lesson": "run tests before push",
            "context": "ci lane 3",
            "bank": "bank-a"
        }),
    );
    let failures = read(
        &state,
        "failures_for_task",
        &json!({ "task_id_or_query": "t-1", "bank": "bank-a" }),
    );
    let arr = failures["failures"].as_array().expect("failures array");
    assert_eq!(arr[0]["context"], "ci lane 3");
}

#[test]
fn triple_add_and_belief_observe_context_round_trips() {
    let state = state();
    write(
        &state,
        "triple_add",
        json!({
            "subject": "release",
            "predicate": "targets",
            "object": "v2",
            "epistemic_kind": "fact",
            "context": "planning notes",
            "bank": "bank-a"
        }),
    );
    let traj = read(
        &state,
        "temporal_history",
        &json!({ "target": "triple:release:targets", "bank": "bank-a" }),
    );
    let body = serde_json::to_string(&traj).expect("serialize trajectory");
    assert!(
        body.contains("planning notes"),
        "context must surface: {traj}"
    );

    write(
        &state,
        "belief_observe",
        json!({
            "proposition": "The indexer keeps lexical-first ordering",
            "confidence": 0.8,
            "evidence_ref": "turn_7",
            "source_type": "agent_inferred",
            "context": "design review",
            "bank": "bank-a"
        }),
    );
    let history = read(
        &state,
        "belief_history",
        &json!({
            "belief_id_or_proposition": "The indexer keeps lexical-first ordering",
            "bank": "bank-a"
        }),
    );
    assert_eq!(history["context"], "design review");
}

#[test]
fn oversized_domain_record_context_fails_closed() {
    let state = state();
    state
        .ops_store
        .register_bank("bank-a", "2024-01-01T00:00:00Z")
        .expect("register empty bank fixture");
    let oversized = "a".repeat(4097);
    let out = write(
        &state,
        "persona_put_stable",
        json!({
            "key": "k",
            "value": "v",
            "source": "explicit",
            "context": oversized,
            "bank": "bank-a"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("out_of_range"));
    let doc = persona_get(&state);
    assert!(doc["stable"].as_array().is_none_or(Vec::is_empty));
}
