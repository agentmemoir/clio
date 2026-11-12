// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the stdio transport conformance suite: it
//! drives the write set over stdio and checks stdout purity plus parity against
//! the in-process dispatcher.
//!
//! ## Owns
//! - `run_stdio`, the batch frame driver that asserts each stdout line is one
//!   compact JSON-RPC response.
//! - `stdio_write_set_matrix_with_parity`, the write-set parity test over stdio.
//! - `stdio_removed_handshake_is_unknown_and_notifications_are_silent`, the
//!   removed-method and notification contract over the same transport.
//!
//! ## Does not own
//! - The shared write-set fixture and builders (`main`).
//! - The HTTP transport suite (`http`).
//!
//! ## Boundary
//!
//! Changes to stdio framing or stdout purity should normally require changes
//! here. Changes to the write-set fixture should not require changes here.
//!
//! Keep this module focused on stdio transport conformance.

use clio_mcp::bound_tools;
use serde_json::{Value, json};
use std::io::Cursor;

use crate::{mask_volatile, request, rpc, state, tool_call, tools_list, write_set_fixture};

/// Drive a batch of frames through the stdio transport; every output line is
/// one compact JSON-RPC response (stdout purity is asserted inline).
fn run_stdio(frames: &str) -> Vec<Value> {
    let state = state();
    let mut out: Vec<u8> = Vec::new();
    clio_mcp::stdio::serve(&state, Cursor::new(frames.as_bytes().to_vec()), &mut out)
        .expect("stdio");
    let text = String::from_utf8(out).expect("utf8");
    text.lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("pure stdout line"))
        .collect()
}

/// The whole write set over stdio, with parity against the in-process
/// dispatcher and isError mirroring gated rejects.
#[test]
fn stdio_write_set_matrix_with_parity() {
    let fixtures = write_set_fixture();
    let direct_state = state();
    let direct: Vec<Value> = fixtures
        .iter()
        .map(|(name, args)| {
            clio_mcp::write_tools::dispatch_in_scope(
                &direct_state,
                name,
                args,
                clio_mcp::ToolScope::Advertised,
            )
        })
        .collect();
    let mut frames = String::new();
    for (i, (name, args)) in fixtures.iter().enumerate() {
        frames.push_str(&tool_call(100 + i as u64, name, args));
        frames.push('\n');
    }
    frames.push_str(&tools_list(300));
    frames.push('\n');
    let responses = run_stdio(&frames);
    assert_eq!(
        responses.len(),
        fixtures.len() + 1,
        "every fixture answered (stdout purity)"
    );
    let tools_list = responses
        .iter()
        .find(|r| r["id"].as_u64() == Some(300))
        .expect("tools/list response");
    let tools = tools_list["result"]["tools"].as_array().expect("tools");
    let bound = bound_tools();
    let expected: Vec<&str> = clio_types::ToolPreset::All
        .tools()
        .into_iter()
        .filter(|name| bound.contains(name))
        .collect();
    assert_eq!(
        tools.len(),
        expected.len(),
        "the selected profile's bound tools are listed"
    );
    for name in expected {
        assert!(tools.iter().any(|t| t["name"].as_str() == Some(name)));
    }
    for (i, (name, _)) in fixtures.iter().enumerate() {
        let response = responses
            .iter()
            .find(|r| r["id"].as_u64() == Some(100 + i as u64))
            .unwrap_or_else(|| panic!("response for fixture `{name}`"));
        let over_stdio = response["result"]["structuredContent"].clone();
        assert_eq!(
            mask_volatile(over_stdio),
            mask_volatile(direct[i].clone()),
            "stdio/direct parity mismatch on `{name}`"
        );
        let is_error = response["result"]["isError"].as_bool().expect("isError");
        let ok = direct[i]["ok"].as_bool().expect("ok");
        assert_eq!(is_error, !ok, "isError mirrors ok for `{name}`");
    }
}

/// The revision removed the handshake: `initialize` and `ping` answer as
/// unknown methods, a notification is served silently, and a client that sends
/// the retired revision is refused with the unsupported-version code.
#[test]
fn stdio_removed_handshake_is_unknown_and_notifications_are_silent() {
    let mut frames = String::new();
    frames.push_str(&request(1, "initialize"));
    frames.push('\n');
    frames.push_str(&request(2, "ping"));
    frames.push('\n');
    frames.push_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    frames.push('\n');
    frames.push_str(&rpc(
        3,
        "server/discover",
        json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-11-25",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }),
    ));
    frames.push('\n');
    let responses = run_stdio(&frames);
    assert_eq!(
        responses.len(),
        3,
        "three requests answered and the notification silent: {responses:?}"
    );
    for (index, method) in [(0, "initialize"), (1, "ping")] {
        assert_eq!(
            responses[index]["error"]["code"],
            json!(-32601),
            "`{method}` was removed from the revision"
        );
        assert!(responses[index].get("result").is_none(), "{method} served");
    }
    let refused = &responses[2];
    assert_eq!(
        refused["error"]["code"],
        json!(-32022),
        "the retired revision is refused: {refused}"
    );
    assert_eq!(
        refused["error"]["data"]["supported"],
        json!([clio_mcp::MCP_PROTOCOL_REVISION])
    );
    assert_eq!(refused["error"]["data"]["requested"], json!("2025-11-25"));
}
