// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the additive-tools conformance suite: an
//! end-to-end JSON-RPC transcript driving the additive harness workspace tools
//! over the shared MCP protocol handler.
//!
//! ## Owns
//! - JSON-RPC transcript assertions: batch atomic commit + rollback, scratchpad
//!   isolation, canonical supersession, shared-bank gating, validate routing,
//!   stats accuracy, and additive tool schema publication
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
        database_url: format!("/tmp/am-additive-{}-{n}-{shared}.db", std::process::id()),
        default_bank: "bank-a".into(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: shared,
        kms_path: None,
    })
    .expect("state");
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

fn store_op(id: &str, marker: &str) -> Value {
    json!({
        "tool": "store",
        "params": {
            "item": {
                "id": id,
                "subject_id": format!("subj-{id}"),
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

/// Direct `store` tool args (not the batch-op wrapper shape).
fn store_tool_args(id: &str, marker: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": format!("subj-{id}"),
            "gist": format!("{marker} prefers terse answers"),
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers terse answers ({marker}, 1 note since 2024-01-01)."),
        "bank": "bank-a"
    })
}

fn bank_count(h: &McpHandler, bank: &str) -> usize {
    h.state()
        .store
        .count_items(&clio_store::ItemInspectFilter::new(bank))
        .unwrap()
}

#[test]
fn additive_tools_are_registered_and_published() {
    let h = handler(false);
    let listed = h
        .handle_message(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .expect("response")["result"]["tools"]
        .as_array()
        .expect("tools")
        .clone();
    let names: Vec<&str> = listed.iter().filter_map(|t| t["name"].as_str()).collect();
    for tool in [
        "batch",
        "scratchpad_write",
        "scratchpad_read",
        "scratchpad_clear",
        "canonical_put",
        "canonical_get",
        "shared_store",
        "shared_retrieve",
        "shared_discard",
        "validate",
        "stats",
    ] {
        assert!(
            names.contains(&tool),
            "additive tool `{tool}` not published"
        );
    }
}

#[test]
fn batch_atomic_commit_transcript() {
    let h = handler(false);
    let out = call(
        &h,
        "batch",
        &json!({"operations": [store_op("itm-1", "alpha"), store_op("itm-2", "beta")]}),
    );
    assert_eq!(out["isError"], json!(false), "{out}");
    let structured = &out["structuredContent"];
    assert_eq!(structured["operations"][0]["status"], "COMMITTED");
    assert_eq!(structured["operations"][1]["status"], "COMMITTED");
    assert_eq!(bank_count(&h, "bank-a"), 2);
}

#[test]
fn batch_rollback_transcript_leaves_zero_rows() {
    let h = handler(false);
    let out = call(
        &h,
        "batch",
        &json!({
            "operations": [
                store_op("itm-1", "alpha"),
                store_op("itm-2", "beta"),
                {"tool": "unknown_tool", "params": {}}
            ]
        }),
    );
    assert_eq!(out["isError"], json!(true), "{out}");
    let structured = &out["structuredContent"];
    assert!(
        structured["message"]
            .as_str()
            .unwrap_or("")
            .contains("unrecognized batch op tool"),
        "{structured}"
    );
    // Verification rule: row count diff == 0 after the aborted batch.
    assert_eq!(bank_count(&h, "bank-a"), 0);
}

#[test]
fn scratchpad_isolated_from_long_term_storage() {
    let h = handler(false);
    let before = bank_count(&h, "bank-a");
    let write = call(
        &h,
        "scratchpad_write",
        &json!({"key": "note", "content": "zqxj probe", "bank": "bank-a"}),
    );
    assert_eq!(write["structuredContent"]["ok"], json!(true), "{write}");
    // Scratchpad content never becomes a durable row.
    assert_eq!(bank_count(&h, "bank-a"), before);
    // Cross-session scope is isolated.
    let missing = call(
        &h,
        "scratchpad_read",
        &json!({"key": "note", "session": "other", "bank": "other-bank"}),
    );
    assert_eq!(missing["structuredContent"]["ok"], json!(false));
}

#[test]
fn canonical_supersession_transcript() {
    let h = handler(false);
    for value in ["db-a", "db-b"] {
        let out = call(
            &h,
            "canonical_put",
            &json!({
                "key": "primary_host", "value": value, "category": "tool_config",
                "epistemic_kind": "fact", "bank": "bank-a"
            }),
        );
        assert_eq!(out["isError"], json!(false), "{out}");
        assert!(out["structuredContent"]["pass"].as_bool().unwrap_or(false));
    }
    let get = call(
        &h,
        "canonical_get",
        &json!({"key": "primary_host", "bank": "bank-a"}),
    );
    assert_eq!(get["structuredContent"]["value"], "db-b");
    // Exactly one active revision remains, and one superseded row is retained
    // (supersession closes both time axes, nothing is deleted).
    assert_eq!(bank_count(&h, "bank-a"), 2);
}

#[test]
fn shared_bank_gates() {
    let disabled = handler(false);
    let forbidden = call(
        &disabled,
        "shared_store",
        &json!({"item": {"id": "x"}, "epistemic_kind": "fact", "category": "schema", "actor": "agent"}),
    );
    assert_eq!(forbidden["structuredContent"]["ok"], json!(false));
    assert_eq!(forbidden["structuredContent"]["code"], json!("forbidden"));

    let h = handler(true);
    let stored = call(
        &h,
        "shared_store",
        &json!({
            "item": {"id": "itm-shared", "subject_id": "s", "gist": "shared convention"},
            "category": "tool_config", "epistemic_kind": "fact", "actor": "agent"
        }),
    );
    assert_eq!(stored["isError"], json!(false), "{stored}");
    assert_eq!(bank_count(&h, "shared"), 1);
}

#[test]
fn validate_actions_transcript() {
    let h = handler(false);
    let stored = call(&h, "store", &store_tool_args("itm-v", "verify"));
    assert_eq!(stored["isError"], json!(false), "{stored}");

    // Attest: telemetry only, content unchanged.
    let attest = call(
        &h,
        "validate",
        &json!({"item_id": "itm-v", "action": "attest", "note": "checked", "bank": "bank-a"}),
    );
    assert_eq!(attest["isError"], json!(false), "{attest}");
    assert_eq!(attest["structuredContent"]["applied"], json!(false));

    // Destructive without a note fails closed.
    let destructive = call(
        &h,
        "validate",
        &json!({"item_id": "itm-v", "action": "invalidate", "bank": "bank-a"}),
    );
    assert_eq!(destructive["structuredContent"]["ok"], json!(false));

    // Invalidate with a note closes both axes.
    let invalidated = call(
        &h,
        "validate",
        &json!({"item_id": "itm-v", "action": "invalidate", "note": "stale", "bank": "bank-a"}),
    );
    assert_eq!(
        invalidated["structuredContent"]["ok"],
        json!(true),
        "{invalidated}"
    );
    let item = h
        .state()
        .store
        .get_memory_item("bank-a", "itm-v")
        .unwrap()
        .unwrap();
    assert!(item.tx_until.is_some());
    // Reviewer note is recorded in durable telemetry.
    let trail = h.state().store.audit_trail("bank-a", "itm-v").unwrap();
    assert!(
        trail
            .events
            .iter()
            .any(|e| e.operation == "validate_invalidate" && e.detail.to_string().contains("stale"))
    );
}

#[test]
fn stats_reports_bank_scoped_counts() {
    let h = handler(false);
    let stored = call(&h, "store", &store_tool_args("itm-stats", "stats"));
    assert_eq!(stored["isError"], json!(false));
    let report = call(&h, "stats", &json!({"bank": "bank-a"}));
    assert_eq!(report["structuredContent"]["bank"], "bank-a");
    assert!(
        report["structuredContent"]["items_total"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );
    let empty = call(&h, "stats", &json!({"bank": "empty-bank"}));
    assert_eq!(empty["structuredContent"]["items_total"], json!(0));
}
