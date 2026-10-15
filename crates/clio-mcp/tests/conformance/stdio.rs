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

use crate::{handler, init_frame, mask_volatile, rpc, state, write_set_fixture};

/// Drive a batch of frames through the stdio transport; every output line is
/// one compact JSON-RPC response (stdout purity is asserted inline).
fn run_stdio(frames: &str) -> Vec<Value> {
    let h = handler();
    let mut out: Vec<u8> = Vec::new();
    clio_mcp::stdio::serve(&h, Cursor::new(frames.as_bytes().to_vec()), &mut out).expect("stdio");
    let text = String::from_utf8(out).expect("utf8");
    text.lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("pure stdout line"))
        .collect()
}

/// the whole write set over stdio, with parity
/// against the in-process dispatcher and isError mirroring gated rejects.
#[test]
fn stdio_write_set_matrix_with_parity() {
    let fixtures = write_set_fixture();
    let direct_state = state();
    let direct: Vec<Value> = fixtures
        .iter()
        .map(|(name, args)| clio_mcp::write_tools::dispatch(&direct_state, name, args))
        .collect();
    let mut frames = init_frame();
    for (i, (name, args)) in fixtures.iter().enumerate() {
        frames.push_str(&rpc(
            100 + i as u64,
            "tools/call",
            json!({"name": name, "arguments": args}),
        ));
        frames.push('\n');
    }
    frames.push_str(&rpc(300, "tools/list", json!({})));
    frames.push('\n');
    let responses = run_stdio(&frames);
    assert_eq!(
        responses.len(),
        fixtures.len() + 2,
        "initialize + every fixture answered (stdout purity)"
    );
    let tools_list = responses
        .iter()
        .find(|r| r["id"].as_u64() == Some(300))
        .expect("tools/list response");
    let tools = tools_list["result"]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), bound_tools().len(), "bound tool set listed");
    for bound in bound_tools() {
        assert!(tools.iter().any(|t| t["name"].as_str() == Some(bound)));
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
