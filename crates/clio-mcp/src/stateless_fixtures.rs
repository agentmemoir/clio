// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures of the candidate
//! `2026-07-28` dispatcher's unit tests: runtime state, request metadata, and
//! the tool-call and answer shapes those fixtures send.
//!
//! ## Owns
//! - The fixture runtime state and its registered bank
//! - The request envelopes and `_meta` variants the candidate tests send
//! - Accessors for a response's error code, error message, and server identity
//!
//! ## Does not own
//! - Any assertion (see `stateless_tests`)
//! - The dispatcher itself (see `stateless`)
//!
//! ## Boundary
//!
//! Changes to how the candidate tests build a request should normally require
//! changes here. Changes to what a fixture asserts should not.
//!
//! Keep this module free of `#[test]` functions.

use serde_json::{Map, Value, json};
use std::sync::Arc;

use crate::runtime::{McpOpenOptions, McpState};

pub(super) const BANK: &str = "fixture-bank";

pub(super) fn state() -> Arc<McpState> {
    let state =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    state
        .ops_store
        .register_bank(BANK, "2024-01-01T00:00:00Z")
        .expect("bank fixture");
    state
}

/// Request metadata carrying the two keys the revision requires.
pub(super) fn required_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// Request metadata declaring the client's elicitation capability.
pub(super) fn eliciting_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": { "form": {} } }
    })
}

/// Required metadata plus both optional keys the revision defines.
pub(super) fn full_meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "fixture-client", "version": "0.0.1" },
        "io.modelcontextprotocol/logLevel": "info"
    })
}

pub(super) fn request(method: &str, meta: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": { "_meta": meta } })
}

pub(super) fn tool_call(meta: &Value, extra: Value) -> Value {
    let mut params = Map::new();
    params.insert("_meta".into(), meta.clone());
    params.insert("name".into(), json!("consolidate"));
    params.insert("arguments".into(), json!({ "bank": BANK }));
    if let Value::Object(extra) = extra {
        params.extend(extra);
    }
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": params })
}

/// An answer the revision types as `ElicitResult`, whose `action` is required.
pub(super) fn elicitation_answer() -> Value {
    json!({ "inputResponses": { "ask-1": { "action": "accept", "content": {} } } })
}

/// An answer the revision types as `CreateMessageResult`: no `action` member.
pub(super) fn sampling_answer() -> Value {
    json!({ "inputResponses": { "ask-1": {
        "role": "assistant",
        "content": { "type": "text", "text": "fixture" },
        "model": "fixture-model"
    } } })
}

pub(super) fn code(res: &Value) -> i64 {
    res["error"]["code"].as_i64().expect("an error code")
}

pub(super) fn message(res: &Value) -> &str {
    res["error"]["message"].as_str().expect("an error message")
}

pub(super) fn server_info(result: &Value) -> &Value {
    &result["_meta"]["io.modelcontextprotocol/serverInfo"]
}
