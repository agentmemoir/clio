// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the entry point of the MCP conformance
//! integration-test target for the pinned "2025-11-25" revision and for the
//! shared in-process harness every transport suite builds on.
//!
//! ## Owns
//! - The `conformance` Cargo test target and its module declarations.
//! - The shared `McpState` and `McpHandler` builders, the JSON-RPC frame helper,
//!   the initialize frames, and the write-set fixture.
//! - `mask_volatile`, the parity mask over process-unique generated ids.
//!
//! ## Does not own
//! - The pinned-revision assertions (`revision`).
//! - The stdio transport suite (`stdio`).
//! - The HTTP transport suite (`http`).
//!
//! ## Boundary
//!
//! Changes to the shared write-set fixture or the in-process builders should
//! normally require changes here. Changes to one transport's assertions should
//! not require changes here.
//!
//! Keep this module focused on the target entry point and its shared harness.
//
// Run locally with `cargo test -p clio-mcp --test conformance`.

use clio_mcp::MCP_PROTOCOL_REVISION;
use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::sync::Arc;

mod http;
mod revision;
mod stdio;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: "conf-bank".into(),
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("state")
}

fn handler() -> McpHandler {
    McpHandler::new(state())
}

#[allow(clippy::needless_pass_by_value)]
fn rpc(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string()
}

/// The initialize POST body (a single JSON-RPC message).
fn init_only() -> String {
    rpc(
        1,
        "initialize",
        json!({"protocolVersion": MCP_PROTOCOL_REVISION, "capabilities": {}}),
    )
}

fn init_frame() -> String {
    format!(
        "{}\n{}\n",
        rpc(
            1,
            "initialize",
            json!({"protocolVersion": MCP_PROTOCOL_REVISION, "capabilities": {}})
        ),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
    )
}

/// The write-set fixture: happy paths plus gated-reject samples.
///  Fixed timestamps keep transport parity deterministic.
fn write_set_fixture() -> Vec<(&'static str, Value)> {
    let ts = "2024-01-01T00:00:00Z";
    let ts2 = "2024-01-02T00:00:00Z";
    vec![
        (
            "admit_preview",
            json!({
                "item": {"id": "cf-1", "subject_id": "cf-subj", "gist": "g",
                         "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}},
                "category": "persona", "epistemic_kind": "fact",
                "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
                "timestamp": ts
            }),
        ),
        (
            "store",
            json!({
                "item": {"id": "cf-store-1", "subject_id": "cf-subj", "gist": "g",
                         "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}},
                "category": "persona", "epistemic_kind": "fact",
                "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
                "timestamp": ts
            }),
        ),
        (
            "store",
            json!({
                "item": {"id": "cf-bad", "gist": "g"},
                "epistemic_kind": "fact",
                "timestamp": ts
            }),
        ),
        (
            "update",
            json!({"target": "cf-store-1", "new_value": "1", "update_rule": "discrete", "timestamp": ts2}),
        ),
        (
            "update",
            json!({"target": "cf-store-1", "new_value": "1", "update_rule": "fact", "timestamp": ts2}),
        ),
        (
            "invalidate",
            json!({"item_id": "cf-store-1", "timestamp": ts2}),
        ),
        (
            "discard",
            json!({"item_id": "cf-store-1", "reason": "ops cleanup", "timestamp": ts2}),
        ),
        (
            "discard",
            json!({"item_id": "cf-store-1", "reason": "ops cleanup", "confirm": true, "timestamp": ts2}),
        ),
        ("consolidate", json!({"force": true, "timestamp": ts2})),
        (
            "persona_put_stable",
            json!({"key": "style", "value": "terse", "timestamp": ts}),
        ),
        (
            "persona_put_stable",
            json!({"key": "verbosity", "value": "0.9", "timestamp": ts}),
        ),
        (
            "persona_observe_preference",
            json!({"key": "verbosity", "observed_value": 0.9, "timestamp": ts}),
        ),
        (
            "task_upsert",
            json!({"task_id": "cf-task", "definition": "Run conformance", "status": "running", "timestamp": ts}),
        ),
        (
            "failure_record",
            json!({"task_id": "cf-task", "attempt_n": 1, "what_failed": "flake", "lesson": "retry once", "timestamp": ts}),
        ),
        (
            "triple_add",
            json!({"subject": "rust", "predicate": "used_by", "object": "agent", "epistemic_kind": "fact", "timestamp": ts}),
        ),
        (
            "triple_add",
            json!({"subject": "rust", "predicate": "used_by", "object": "agent", "epistemic_kind": "fact", "update_rule": "continuous", "timestamp": ts}),
        ),
        (
            "triple_end",
            json!({"subject": "rust", "predicate": "used_by", "valid_until": ts2, "timestamp": ts2}),
        ),
        (
            "belief_observe",
            json!({"proposition": "Friday deploys are risky", "confidence": 0.7, "evidence_ref": "ev-1", "source_type": "user_stated", "timestamp": ts}),
        ),
        (
            "belief_observe",
            json!({"proposition": "Friday deploys are risky", "confidence": 0.9, "evidence_ref": "ev-2", "source_type": "agent_inferred", "timestamp": ts2}),
        ),
        (
            "graph_link",
            json!({"source_id": "cf-store-1", "target_id": "cf-task", "relationship": "relates_to", "timestamp": ts2}),
        ),
    ]
}

/// Mask process-unique generated ids before comparing transport parity.
fn mask_volatile(value: Value) -> Value {
    const VOLATILE: [&str; 8] = [
        "new_edge_id",
        "prior_edge_ids",
        "closed_ids",
        "id",
        "item_id",
        "record_id",
        "belief_id",
        "replacement_id",
    ];
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    if VOLATILE.contains(&k.as_str()) {
                        (k, Value::from("<volatile>"))
                    } else {
                        (k, mask_volatile(v))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(mask_volatile).collect()),
        other => other,
    }
}
