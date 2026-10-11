// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Bank existence checks shared by bank-scoped MCP reads.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    let options = McpOpenOptions {
        default_bank: "bank-default".into(),
        ..McpOpenOptions::default()
    };
    McpState::open_with_effective(&options, &json!({})).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_item(state: &McpState) {
    let result = call(
        state,
        "store",
        json!({
            "item": {
                "id": "known-bank-item",
                "subject_id": "known-bank-subject",
                "gist": "known bank content",
                "snapshot": {"entity": "known bank content"}
            },
            "category": "tool_config",
            "epistemic_kind": "fact",
            "source_text": "known bank content",
            "bank": "known-bank",
            "actor": "agent",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(result["ok"], json!(true), "{result}");
}

#[test]
fn unknown_bank_reads_error_and_registered_empty_default_remains_readable() {
    let state = state();
    for (name, args) in [
        ("stats", json!({"bank": "typo-bank"})),
        (
            "retrieve",
            json!({"bank": "typo-bank", "query": "anything"}),
        ),
        ("inspect", json!({"bank": "typo-bank"})),
        ("verify", json!({"bank": "typo-bank"})),
        ("doctor", json!({"bank": "typo-bank"})),
    ] {
        let result = call(&state, name, args);
        assert_eq!(result["ok"], json!(false), "{name}: {result}");
        assert_eq!(result["code"], json!("not_found"), "{name}: {result}");
    }

    let empty = call(&state, "stats", json!({"bank": "bank-default"}));
    assert_eq!(empty["ok"], json!(true), "{empty}");
}

#[test]
fn first_memory_write_registers_a_bank_for_empty_and_populated_reads() {
    let mcp = state();
    store_item(&mcp);

    let statistics = call(&mcp, "stats", json!({"bank": "known-bank"}));
    assert_eq!(statistics["ok"], json!(true), "{statistics}");
    let inspect = call(&mcp, "inspect", json!({"bank": "known-bank"}));
    assert_eq!(inspect["ok"], json!(true), "{inspect}");
    assert_eq!(inspect["items"][0]["item_id"], json!("known-bank-item"));
}
