// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the extended additive-tools conformance
//! suite: a JSON-RPC transcript exercising every recognized `batch` operation
//! type, the ephemeral scratchpad clear path, and the cross-agent shared
//! surface (`shared_retrieve` / `shared_discard`) over the shared MCP handler.
//!
//! ## Owns
//! - Transcript assertions for mixed-op batches, admission rollback, and the
//!   additive tool paths not covered by the primary conformance suite
//!
//! ## Does not own
//! - In-process semantics (domain crates own behavior)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::sync::Arc;

fn handler(shared: bool) -> Arc<McpHandler> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let state = McpState::open(&McpOpenOptions {
        backend: "sqlite".into(),
        database_url: format!("/tmp/am-additive-x-{}-{n}-{shared}.db", std::process::id()),
        default_bank: "bank-a".into(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: shared,
        kms_path: None,
    })
    .expect("state");
    Arc::new(McpHandler::new(state))
}

/// Handler whose admission threshold rejects every candidate (θ > 1.0).
fn rejecting_handler() -> Arc<McpHandler> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut state = McpState::open(&McpOpenOptions {
        backend: "sqlite".into(),
        database_url: format!("/tmp/am-additive-rej-{}-{n}.db", std::process::id()),
        default_bank: "bank-a".into(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path: None,
    })
    .expect("state");
    Arc::get_mut(&mut state).expect("single owner").env = clio_config::ranking::RankingEnv {
        theta_admit: 1.1,
        ..clio_config::ranking::RankingEnv::system_defaults()
    };
    Arc::new(McpHandler::new(state))
}

fn call(h: &McpHandler, name: &str, args: &Value) -> Value {
    let response = h
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {"name": name, "arguments": args}
        }))
        .expect("response");
    response["result"].clone()
}

fn bank_count(h: &McpHandler, bank: &str) -> usize {
    h.state()
        .store
        .count_items(&clio_store::ItemInspectFilter::new(bank))
        .unwrap()
}

/// A `store` op whose params omit `item.id` (the engine assigns one).
fn store_op_auto(marker: &str) -> Value {
    json!({
        "tool": "store",
        "params": {
            "item": {
                "subject_id": format!("subj-{marker}"),
                "gist": format!("{marker} prefers terse answers"),
                "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": format!("The user prefers terse answers ({marker}, 1 note since 2024-01-01)."),
            "bank": "bank-a"
        }
    })
}

#[test]
fn batch_mixed_op_types_commit_transcript() {
    let h = handler(false);
    let out = call(
        &h,
        "batch",
        &json!({"operations": [
            store_op_auto("mixed-auto"),
            {"tool": "triple_add", "params": {
                "subject": "alice", "predicate": "lives_in", "object": "boston",
                "epistemic_kind": "fact", "confidence": 0.9, "bank": "bank-a"
            }},
            {"tool": "canonical_put", "params": {
                "key": "primary_host", "value": "db-a", "category": "tool_config",
                "epistemic_kind": "fact", "bank": "bank-a", "item": {"id": "itm-canon-op"}
            }},
            {"tool": "persona_put_stable", "params": {
                "key": "communication_style", "value": "concise", "confidence": 0.8,
                "bank": "bank-a"
            }},
            {"tool": "task_upsert", "params": {
                "task_id": "t-1", "definition": "fix the build", "steps": ["edit", "test"],
                "status": "in_progress", "bank": "bank-a"
            }},
            {"tool": "failure_record", "params": {
                "task_id": "t-1", "attempt_n": 1, "what_failed": "compile error",
                "lesson": "run cargo check first", "bank": "bank-a"
            }},
            {"tool": "graph_link", "params": {
                "source_id": "$ref:0.id", "target_id": "$ref:1.id",
                "relationship": "related", "bank": "bank-a"
            }}
        ]}),
    );
    assert_eq!(out["isError"], json!(false), "{out}");
    let ops = out["structuredContent"]["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 7);
    for op in ops {
        assert_eq!(op["status"], "COMMITTED", "{op}");
    }
    let state = h.state();
    assert_eq!(
        state.store.list_assoc_edges("bank-a", None).unwrap().len(),
        1
    );
    assert_eq!(state.history.list_tasks("bank-a", 10).unwrap().len(), 1);
    assert_eq!(state.history.list_failures("bank-a", 10).unwrap().len(), 1);
    let stable = state.persona.get_persona_document("bank-a").unwrap().stable;
    assert!(!stable.is_empty(), "{stable:?}");
}

#[test]
fn batch_admission_rejection_transcript_rolls_back() {
    let h = rejecting_handler();
    let out = call(
        &h,
        "batch",
        &json!({"operations": [store_op_auto("reject")]}),
    );
    assert_eq!(out["isError"], json!(true), "{out}");
    assert!(
        out["structuredContent"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("operation 0"),
        "{out}"
    );
    assert_eq!(bank_count(&h, "bank-a"), 0);

    // A record-shaped op under the failing θ aborts through the same gate.
    let rejected = call(
        &h,
        "batch",
        &json!({"operations": [{"tool": "task_upsert", "params": {
            "task_id": "t-rej", "definition": "fix the build", "steps": ["edit"],
            "status": "in_progress", "bank": "bank-a"
        }}]}),
    );
    assert_eq!(rejected["isError"], json!(true), "{rejected}");
    assert_eq!(bank_count(&h, "bank-a"), 0);
}

#[test]
fn batch_failure_record_requires_attempt_n() {
    let h = handler(false);
    let out = call(
        &h,
        "batch",
        &json!({"operations": [{"tool": "failure_record", "params": {
            "task_id": "t-2", "what_failed": "compile error",
            "lesson": "run cargo check first", "bank": "bank-a"
        }}]}),
    );
    assert_eq!(out["isError"], json!(true), "{out}");
    assert!(
        out["structuredContent"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("attempt_n"),
        "{out}"
    );
}

#[test]
fn canonical_put_with_explicit_item_id_transcript() {
    let h = handler(false);
    let out = call(
        &h,
        "canonical_put",
        &json!({
            "key": "explicit_slot", "value": "v1", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "bank-a", "item": {"id": "itm-canon"}
        }),
    );
    assert_eq!(out["isError"], json!(false), "{out}");
    assert_eq!(out["structuredContent"]["id"], json!("itm-canon"));
}

#[test]
fn scratchpad_clear_and_shared_tool_transcript() {
    let h = handler(true);
    let write = call(
        &h,
        "scratchpad_write",
        &json!({"key": "k1", "content": "c1", "bank": "bank-a"}),
    );
    assert_eq!(write["structuredContent"]["ok"], json!(true), "{write}");
    let cleared = call(
        &h,
        "scratchpad_clear",
        &json!({"key": "k1", "bank": "bank-a"}),
    );
    assert_eq!(cleared["structuredContent"]["ok"], json!(true), "{cleared}");
    let view = call(&h, "scratchpad_read", &json!({"bank": "bank-a"}));
    assert_eq!(view["structuredContent"]["total_bytes"], json!(0));

    let stored = call(
        &h,
        "shared_store",
        &json!({
            "item": {"id": "itm-sh2", "subject_id": "s", "gist": "shared convention two"},
            "category": "tool_config", "epistemic_kind": "fact", "actor": "agent"
        }),
    );
    assert_eq!(stored["isError"], json!(false), "{stored}");
    let hits = call(
        &h,
        "shared_retrieve",
        &json!({"query": "shared convention two", "actor": "agent"}),
    );
    assert_eq!(hits["isError"], json!(false), "{hits}");
    let discarded = call(
        &h,
        "shared_discard",
        &json!({
            "item_id": "itm-sh2", "reason": "convention retired", "confirm": true, "actor": "agent"
        }),
    );
    assert_eq!(discarded["isError"], json!(false), "{discarded}");
}
