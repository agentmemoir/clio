// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! This module is responsible for testing stdio framing, stdout purity, and a
//! literal store-to-retrieve exchange.
//!
//! ## Owns
//! - Newline framing, session, EOF, and stdout-purity assertions
//! - Literal store-to-retrieve evidence through the stdio serving loop
//!
//! ## Does not own
//! - Production stdio framing and output behavior (see `stdio.rs`)
//! - JSON-RPC protocol or tool semantics (see `protocol.rs` and tool modules)
//!
//! ## Boundary
//!
//! Changes to stdio transport test coverage should normally require changes here.
//! Changes to framing implementation or protocol semantics should not require changes here.
//!
//! Keep this module focused on stdio transport tests.

use super::serve;
use crate::protocol::McpHandler;
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::io::{self, BufRead, Cursor, Read};
use std::sync::Arc;

struct DrainBeforeRetrieve {
    input: Vec<u8>,
    state: Arc<McpState>,
    position: usize,
    drain_on_next_read: bool,
}

impl DrainBeforeRetrieve {
    fn new(state: Arc<McpState>, input: Vec<u8>) -> Self {
        Self {
            input,
            state,
            position: 0,
            drain_on_next_read: false,
        }
    }
}

impl Read for DrainBeforeRetrieve {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let len = available.len().min(output.len());
        output[..len].copy_from_slice(&available[..len]);
        self.consume(len);
        Ok(len)
    }
}

impl BufRead for DrainBeforeRetrieve {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.drain_on_next_read {
            self.state
                .drain_index()
                .map_err(|error| io::Error::other(error.to_string()))?;
            self.drain_on_next_read = false;
        }
        let remaining = &self.input[self.position..];
        if remaining.is_empty() {
            return Ok(&[]);
        }
        let line_len = remaining
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(remaining.len(), |offset| offset + 1);
        Ok(&remaining[..line_len])
    }

    fn consume(&mut self, amount: usize) {
        let next = (self.position + amount).min(self.input.len());
        if next > self.position && self.input[..next].ends_with(b"\n") {
            self.drain_on_next_read = true;
        }
        self.position = next;
    }
}

fn assert_compact_json(line: &str) {
    let mut in_string = false;
    let mut escaped = false;
    for character in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else {
            assert!(
                !character.is_ascii_whitespace(),
                "JSON response contains formatting whitespace: {line:?}"
            );
            if character == '"' {
                in_string = true;
            }
        }
    }
    assert!(!in_string && !escaped, "unterminated JSON string: {line:?}");
}

fn frames() -> String {
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {}}
    });
    let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
    let store = json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "store", "arguments": {
            "item": {"id": "itm-s1", "subject_id": "subj-s1", "gist": "g"},
            "category": "persona", "epistemic_kind": "fact",
            "source_text": "The user prefers terse answers.",
            "snapshot": {"gist": "prefers terse answers"},
            "bank": "bank-a"
        }}
    });
    let notify = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let empty_line = "";
    format!("{init}\n{list}\n{notify}\n{empty_line}\n{store}\n")
}

#[test]
fn t04_stdio_session_flows_end_to_end() {
    let state = crate::runtime::McpState::open(&McpOpenOptions::default()).expect("state");
    let handler = McpHandler::new(state);
    let input = Cursor::new(frames().into_bytes());
    let mut out: Vec<u8> = Vec::new();
    serve(&handler, input, &mut out).expect("serve");
    let text = String::from_utf8(out).expect("utf8");
    // Every output line is exactly one JSON-RPC response (no embedded newlines).
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "notification and blank line produce no output: {text:?}"
    );
    for line in &lines {
        let v: Value = serde_json::from_str(line).expect("valid single-line JSON");
        assert_eq!(v["jsonrpc"], json!("2.0"));
    }
    let init: Value = serde_json::from_str(lines[0]).expect("init");
    assert_eq!(init["result"]["protocolVersion"], json!("2025-11-25"));
    let list: Value = serde_json::from_str(lines[1]).expect("list");
    assert!(list["result"]["tools"].as_array().is_some());
    let store: Value = serde_json::from_str(lines[2]).expect("store");
    assert_eq!(store["result"]["isError"], json!(false));
    assert!(
        store["result"]["structuredContent"]["ok"]
            .as_bool()
            .unwrap_or(false)
    );
}

#[test]
fn t13_stdout_purity_no_log_lines() {
    let state = crate::runtime::McpState::open(&McpOpenOptions::default()).expect("state");
    let handler = McpHandler::new(state);
    let input = Cursor::new(frames().into_bytes());
    let mut out: Vec<u8> = Vec::new();
    serve(&handler, input, &mut out).expect("serve");
    let text = String::from_utf8(out).expect("utf8");
    for line in text.lines() {
        let parsed = serde_json::from_str::<Value>(line);
        assert!(parsed.is_ok(), "stdout purity violated by line: {line:?}");
        assert!(parsed.expect("json").get("jsonrpc").is_some());
    }
}

#[test]
fn eof_terminates_cleanly() {
    let state = crate::runtime::McpState::open(&McpOpenOptions::default()).expect("state");
    let handler = McpHandler::new(state);
    let input = Cursor::new(Vec::<u8>::new());
    let mut out: Vec<u8> = Vec::new();
    serve(&handler, input, &mut out).expect("clean EOF");
    assert!(out.is_empty(), "stdout was not empty: {out:?}");
}

#[test]
fn stdio_store_retrieve_end_to_end() {
    let state =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    let handler = McpHandler::new(state.clone());
    let item_id = "itm-stdio-e2e";
    let store = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "store", "arguments": {
            "item": {
                "id": item_id,
                "subject_id": item_id,
                "gist": "prefers terse answers",
                "snapshot": {
                    "entity": "terse answers",
                    "amount": 1,
                    "date": "2024-01-01"
                }
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
            "bank": "bank-a",
            "actor": "agent",
            "timestamp": "2024-01-01T00:00:00Z"
        }}
    });
    let retrieve = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "retrieve", "arguments": {
            "bank": "bank-a",
            "query": "terse answers",
            "limit": 5
        }}
    });
    // Drain between frames so retrieval does not depend on sweeper scheduling.
    let mut input = DrainBeforeRetrieve::new(state, format!("{store}\n{retrieve}\n").into_bytes());
    let mut out: Vec<u8> = Vec::new();
    serve(&handler, &mut input, &mut out).expect("store then retrieve over stdio");

    let text = String::from_utf8(out).expect("utf8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected exactly two response lines: {text:?}"
    );
    assert_eq!(
        text.matches('\n').count(),
        2,
        "unexpected stdout framing: {text:?}"
    );
    assert!(
        text.ends_with('\n'),
        "response framing lacks a final newline"
    );
    for (expected_id, line) in [1, 2].iter().zip(&lines) {
        assert!(!line.contains('\n') && !line.contains('\r'));
        let response: Value = serde_json::from_str(line).expect("valid single-line JSON");
        assert_eq!(response["jsonrpc"], json!("2.0"));
        assert_eq!(response["id"], json!(expected_id));
        assert!(response.get("result").is_some(), "not a response: {line:?}");
        assert_compact_json(line);
    }

    let store_response: Value = serde_json::from_str(lines[0]).expect("store response");
    assert_eq!(store_response["result"]["isError"], json!(false));
    let stored_id = store_response["result"]["structuredContent"]["id"]
        .as_str()
        .expect("stored id");
    assert_eq!(stored_id, item_id);

    let retrieved = &serde_json::from_str::<Value>(lines[1]).expect("retrieve response");
    assert_eq!(retrieved["result"]["isError"], json!(false));
    let hits = retrieved["result"]["structuredContent"]["hits"]
        .as_array()
        .expect("retrieve hits");
    assert!(
        hits.iter().any(|hit| hit["item_id"] == json!(stored_id)),
        "stored id missing from retrieve response: {retrieved}"
    );
}
