// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the fail-closed `context` decode contract on
//! the six domain-record write tools (`persona_put_stable`,
//! `persona_observe_preference`, `task_upsert`, `failure_record`, `triple_add`,
//! `belief_observe`): a present non-string or empty-string value is rejected,
//! a valid string reaches the store, and an absent value persists as none.
//!
//! ## Owns
//! - Per-tool rejection assertions for non-string (number, array) context
//! - Per-tool rejection assertion for explicitly empty-string context
//! - Per-tool valid-string persistence and absent-key omission assertions
//!
//! ## Does not own
//! - Round-trip read-surfacing parity (`context_domain_record_tests`)
//! - Item-level `store`/`admit` context coverage (`context_write_tests`)
//! - The batch preflight decode path (batch preflight record ops)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::read_history;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

/// The six domain-record write tools that bind `context`.
const RECORD_TOOLS: [&str; 6] = [
    "persona_put_stable",
    "persona_observe_preference",
    "task_upsert",
    "failure_record",
    "triple_add",
    "belief_observe",
];

fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
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

/// Minimal schema-conformant args for one record-write tool call; `tag`
/// keeps each call's keys unique within its test.
fn base_args(tool: &str, tag: &str) -> Value {
    match tool {
        "persona_put_stable" => json!({
            "key": format!("style_{tag}"), "value": "concise",
            "source": "explicit", "bank": "bank-a"
        }),
        "persona_observe_preference" => json!({
            "key": format!("pref_{tag}"), "observed_value": 0.9,
            "bank": "bank-a"
        }),
        "task_upsert" => json!({
            "task_id": format!("task-{tag}"), "definition": "Ship the release",
            "status": "open", "bank": "bank-a"
        }),
        "failure_record" => json!({
            "task_id": format!("task-{tag}"), "attempt_n": 1,
            "what_failed": "build broke", "lesson": "test before push",
            "bank": "bank-a"
        }),
        "triple_add" => json!({
            "subject": format!("subj-{tag}"), "predicate": "targets",
            "object": "v1", "epistemic_kind": "fact", "bank": "bank-a"
        }),
        "belief_observe" => json!({
            "proposition": format!("Proposition {tag} is held"),
            "confidence": 0.8, "evidence_ref": "turn_1",
            "source_type": "agent_inferred", "bank": "bank-a"
        }),
        other => unreachable!("not a record-write tool: {other}"),
    }
}

/// Read back the persisted record surface written by one tool call.
fn persisted(state: &McpState, tool: &str, tag: &str) -> Value {
    match tool {
        "persona_put_stable" | "persona_observe_preference" => {
            let doc =
                crate::read_tools::dispatch(state, "persona_get", &json!({ "bank": "bank-a" }))
                    .expect("persona_get succeeds")["document"]
                    .clone();
            let (section, prefix) = if tool == "persona_put_stable" {
                ("stable", "style")
            } else {
                ("preferences", "pref")
            };
            let key = format!("{prefix}_{tag}");
            doc[section]
                .as_array()
                .expect("persona section array")
                .iter()
                .find(|e| e["key"] == key.as_str())
                .cloned()
                .expect("persona entry present")
        }
        "task_upsert" => read(
            state,
            "task_get",
            &json!({ "task_id": format!("task-{tag}"), "bank": "bank-a" }),
        )["record"]
            .clone(),
        "failure_record" => read(
            state,
            "failures_for_task",
            &json!({ "task_id_or_query": format!("task-{tag}"), "bank": "bank-a" }),
        )["failures"][0]
            .clone(),
        "triple_add" => read(
            state,
            "temporal_history",
            &json!({
                "target": format!("triple:subj-{tag}:targets"),
                "bank": "bank-a"
            }),
        ),
        "belief_observe" => read(
            state,
            "belief_history",
            &json!({
                "belief_id_or_proposition": format!("Proposition {tag} is held"),
                "bank": "bank-a"
            }),
        ),
        other => unreachable!("not a record-write tool: {other}"),
    }
}

#[test]
fn non_string_context_is_rejected_for_every_record_tool() {
    let state = state();
    for tool in RECORD_TOOLS {
        for bad in [json!(123), json!(["session notes"])] {
            let mut args = base_args(tool, "rej");
            args["context"] = bad.clone();
            let out = write(&state, tool, args);
            assert_eq!(out["ok"], json!(false), "{tool} context {bad}: {out}");
            assert_eq!(out["code"], json!("invalid_type"), "{tool}: {out}");
        }
    }
}

#[test]
fn empty_string_context_is_rejected_for_every_record_tool() {
    let state = state();
    for tool in RECORD_TOOLS {
        let mut args = base_args(tool, "empty");
        args["context"] = json!("");
        let out = write(&state, tool, args);
        assert_eq!(out["ok"], json!(false), "{tool}: {out}");
        assert_eq!(out["code"], json!("invalid_argument"), "{tool}: {out}");
    }
}

#[test]
fn valid_string_context_reaches_the_store_for_every_record_tool() {
    let state = state();
    for tool in RECORD_TOOLS {
        let mut args = base_args(tool, "ok");
        args["context"] = json!("release week notes");
        let out = write(&state, tool, args);
        assert_eq!(out["ok"], json!(true), "{tool}: {out}");
        let surface = persisted(&state, tool, "ok");
        if tool == "triple_add" {
            let body = serde_json::to_string(&surface).expect("serialize trajectory");
            assert!(body.contains("release week notes"), "{tool}: {surface}");
        } else {
            assert_eq!(
                surface["context"],
                json!("release week notes"),
                "{tool}: {surface}"
            );
        }
    }
}

#[test]
fn absent_context_persists_as_none_for_every_record_tool() {
    let state = state();
    for tool in RECORD_TOOLS {
        let args = base_args(tool, "absent");
        let out = write(&state, tool, args);
        assert_eq!(out["ok"], json!(true), "{tool}: {out}");
        let surface = persisted(&state, tool, "absent");
        let body = serde_json::to_string(&surface).expect("serialize surface");
        assert!(
            !body.contains("\"context\""),
            "{tool} written without context must omit the key: {surface}"
        );
    }
}
