// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the transport-parity test of the retention
//! tools: the same `tools/call` answered over stdio and over Streamable HTTP.
//!
//! ## Owns
//! - The admitted `tools/call` and `tools/list` frames the parity test sends
//! - The parity assertions on the served payloads and on the advertised
//!   profile
//!
//! ## Does not own
//! - The retention tools themselves (`retention_tools`)
//! - The live server and request helpers (`http_test_support_tests`)
//! - The transport gates and the admission (`http`, `stateless`)
//!
//! ## Boundary
//!
//! Changes to how the two transports are compared should normally require
//! changes here. Changes to a gate or to a retention tool should not.
//!
//! Test-only. Never compiled into production builds.

use super::http_test_support_tests::{post_admitted, post_tool_frame, spawn_server};
use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};

/// One `tools/call` frame carrying the metadata the stateless revision
/// requires on every request.
fn call_frame(name: &str, arguments: &Value) -> String {
    json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {
            "_meta": crate::stateless_fixtures::required_meta(crate::MCP_PROTOCOL_REVISION),
            "name": name,
            "arguments": arguments
        }
    })
    .to_string()
}

/// One `tools/list` frame asking for the whole listing, with the required
/// metadata.
fn list_frame() -> String {
    let mut meta = crate::stateless_fixtures::required_meta(crate::MCP_PROTOCOL_REVISION);
    meta["com.agentmemoir/pageSize"] = json!(100);
    json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/list",
        "params": { "_meta": meta }
    })
    .to_string()
}

/// Transport parity for the retention tools: the same `tools/call` returns the
/// same `structuredContent` over stdio and Streamable HTTP.
#[test]
fn retention_tools_parity_stdio_vs_http() {
    let batch = call_frame(
        "admit_preview_batch",
        &json!({
            "bank": "bank-a",
            "items": [
                {"item": {"id": "p1", "gist": "must require decision"}, "episodic_type": "gist", "epistemic_kind": "fact"},
                {"item": {"id": "p2", "gist": "ok"}, "episodic_type": "gist", "epistemic_kind": "fact"}
            ]
        }),
    );
    let profile = call_frame("retention_profile_get", &json!({"bank": "bank-a"}));

    // stdio transport over the shared handler.
    let stdio_state =
        McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
            .expect("state");
    let frames = format!("{batch}\n{profile}\n");
    let mut out: Vec<u8> = Vec::new();
    crate::stdio::serve(
        &stdio_state,
        std::io::Cursor::new(frames.into_bytes()),
        &mut out,
    )
    .expect("serve");
    let stdio: Vec<Value> = String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert_eq!(stdio.len(), 2);

    // Streamable HTTP transport.
    let (port, _) = spawn_server(HttpConfig::default());
    let http_batch: Value =
        serde_json::from_str(&post_tool_frame(port, "admit_preview_batch", &batch, "").body)
            .expect("json");
    let http_profile: Value =
        serde_json::from_str(&post_tool_frame(port, "retention_profile_get", &profile, "").body)
            .expect("json");

    assert_eq!(
        stdio[0]["result"]["structuredContent"],
        http_batch["result"]["structuredContent"]
    );
    assert_eq!(
        stdio[1]["result"]["structuredContent"],
        http_profile["result"]["structuredContent"]
    );
    assert!(
        http_batch["result"]["structuredContent"]["summary"]["would_admit"]
            .as_u64()
            .is_some()
    );

    // Both transports advertise the selected profile's tools in `tools/list`.
    let list = list_frame();
    let http_list: Value =
        serde_json::from_str(&post_admitted(port, "tools/list", &list, "").body).expect("json");
    let mut names: Vec<&str> = http_list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    let bound = crate::bound_tools();
    let mut expected: Vec<&str> = clio_types::DEFAULT_PRESET
        .tools()
        .into_iter()
        .filter(|name| bound.contains(name))
        .collect();
    names.sort_unstable();
    expected.sort_unstable();
    assert_eq!(names, expected, "the listing follows the selected profile");
    assert!(
        !names.contains(&"retention_profile_get"),
        "a tool outside the profile is not advertised over HTTP"
    );
}
