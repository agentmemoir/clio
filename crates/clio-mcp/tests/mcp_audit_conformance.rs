// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This integration test drives the transparency surface over the MCP stdio
//! transport: `correct` (confirmed), `audit_trail`, and `inspect`, verifying
//! the `DoD` claim that these tools work end-to-end over stdio.
//!
//! ## Owns
//! - Stdio round-trip assertions for `correct` / `audit_trail` / `inspect`
//!
//! ## Does not own
//! - In-process service semantics (`clio-compliance`) or storage SQL (`clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::{MCP_PROTOCOL_REVISION, write_tools};
use serde_json::{Value, json};
use std::io::Cursor;
use std::sync::Arc;

const BANK: &str = "audit-conf";
const TS: &str = "2024-01-01T00:00:00Z";

fn fresh_state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: BANK.into(),
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("state")
}

fn seed(state: &McpState) {
    let out = write_tools::dispatch(
        state,
        "store",
        &json!({
            "item": {"id": "itm-1", "subject_id": "subj", "gist": "meeting at 2pm",
                     "snapshot": {"entity": "2pm", "amount": 1, "date": "2024-01-01"}},
            "category": "task_spec", "epistemic_kind": "fact",
            "source_text": "The meeting is at 2pm (1 note since 2024-01-01).",
            "bank": BANK, "actor": "agent", "timestamp": TS
        }),
    );
    assert_eq!(out["ok"], json!(true), "seed: {out}");
}

#[allow(clippy::needless_pass_by_value)]
fn rpc(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string()
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

fn run_stdio(state: &Arc<McpState>, frames: &str) -> Vec<Value> {
    let handler = McpHandler::new(Arc::clone(state));
    let mut out: Vec<u8> = Vec::new();
    clio_mcp::stdio::serve(&handler, Cursor::new(frames.as_bytes().to_vec()), &mut out)
        .expect("stdio");
    String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("pure stdout line"))
        .collect()
}

#[test]
fn stdio_correct_audit_trail_and_inspect_total() {
    let state = fresh_state();
    seed(&state);
    let calls = [
        (
            10,
            json!({"name": "correct", "arguments": {
                "bank": BANK, "item_id": "itm-1", "new_value": {"meeting": "3pm"},
                "reason": "wrong meeting time", "confirm": true,
                "timestamp": "2024-06-01T00:00:00Z", "actor": "user"
            }}),
        ),
        (
            11,
            json!({"name": "audit_trail", "arguments": {"bank": BANK, "item_id": "itm-1"}}),
        ),
        (
            12,
            json!({"name": "inspect", "arguments": {"bank": BANK, "limit": 1}}),
        ),
    ];
    let mut frames = init_frame();
    for (id, args) in &calls {
        frames.push_str(&rpc(*id, "tools/call", args.clone()));
        frames.push('\n');
    }
    let responses = run_stdio(&state, &frames);
    assert_eq!(responses.len(), 4, "stdout purity");

    let corrected = &responses[1]["result"]["structuredContent"];
    assert_eq!(corrected["rule"], json!("discrete"), "{corrected}");

    let trail = &responses[2]["result"]["structuredContent"];
    let ops: Vec<&str> = trail["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter_map(|e| e["operation"].as_str())
        .collect();
    assert!(ops.contains(&"create_memory_item"), "{ops:?}");
    assert!(ops.contains(&"correct"), "{ops:?}");
    let timeline = trail["timeline"].as_array().expect("timeline");
    assert!(!timeline.is_empty(), "merged timeline must not be empty");
    let ts: Vec<&str> = timeline
        .iter()
        .filter_map(|t| t["timestamp"].as_str())
        .collect();
    let mut sorted = ts.clone();
    sorted.sort_unstable();
    assert_eq!(ts, sorted, "timeline must be ordered ascending");

    let inspected = &responses[3]["result"]["structuredContent"];
    assert_eq!(
        inspected["items"].as_array().expect("items").len(),
        1,
        "page is capped at limit"
    );
    assert_eq!(inspected["total"], json!(2), "{inspected}");
}
