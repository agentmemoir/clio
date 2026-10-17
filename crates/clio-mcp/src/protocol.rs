// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared MCP JSON-RPC message handling
//! (initialize handshake, tools/list, tools/call) used by both transports.
//!
//! ## Owns
//! - JSON-RPC 2.0 envelope parsing and response shaping
//! - MCP error-code mapping (tool failures keep machine-readable codes)
//! - The bound write+read tool registration returned by `tools/list`
//!
//! ## Does not own
//! - Tool semantics (see `write_tools`/`mutator_tools`)
//! - Transport framing (see `stdio`/`http`)
//!
//! ## Boundary
//!
//! Changes to MCP protocol handling should normally require changes here.
//! Changes to tool behavior should not require changes here.
//!
//! Keep this module focused on shared MCP protocol handling.

use crate::MCP_PROTOCOL_REVISION;
use crate::runtime::McpState;
use crate::schema::catalog_defs;
use serde_json::{Value, json};
use std::sync::Arc;

/// JSON-RPC parse error.
pub const ERR_PARSE: i64 = -32700;
/// JSON-RPC invalid request.
pub const ERR_INVALID_REQUEST: i64 = -32600;
/// JSON-RPC method not found.
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC invalid params.
pub const ERR_INVALID_PARAMS: i64 = -32602;
/// JSON-RPC internal error.
pub const ERR_INTERNAL: i64 = -32603;
/// Server-defined: co-activation durability handoff failed (fail closed).
pub const ERR_COACTIVATION_HANDOFF: i64 = -32001;

/// Shared MCP message handler over one runtime state.
#[derive(Debug)]
pub struct McpHandler {
    state: Arc<McpState>,
}

impl McpHandler {
    /// Bind the handler to runtime state.
    #[must_use]
    pub fn new(state: Arc<McpState>) -> Self {
        Self { state }
    }

    /// Runtime state accessor (transports may need defaults).
    #[must_use]
    pub fn state(&self) -> &Arc<McpState> {
        &self.state
    }

    /// Pinned protocol revision advertised in initialize responses.
    #[must_use]
    pub fn protocol_revision(&self) -> &'static str {
        MCP_PROTOCOL_REVISION
    }

    /// Handle one raw JSON-RPC message (already framed); `None` when the
    /// message is a notification needing no response.
    #[must_use]
    pub fn handle_message(&self, msg: &Value) -> Option<Value> {
        Self::handle_message_with_state(&self.state, msg)
    }

    fn handle_message_with_state(state: &McpState, msg: &Value) -> Option<Value> {
        // A message without an id is a notification; MCP responses are never
        // sent for notifications, recognized or not.
        let id = msg.get("id")?;
        match Self::route(state, msg) {
            Ok(result) => Some(Self::ok_response(Some(id), &result)),
            Err(err) => Some(Self::error_response(Some(id), &err)),
        }
    }

    /// Handle one raw text frame; invalid JSON maps to a parse error response.
    #[must_use]
    pub fn handle_frame(&self, frame: &str) -> Option<Value> {
        self.handle_frame_with_state(&self.state, frame)
    }

    /// Handle one frame against an explicit state.
    ///
    /// This is the only entry point a credential-bearing transport may use: it
    /// dispatches against a state that already carries the request's principal,
    /// so the identity rule runs before any tool body. `handle_frame` delegates
    /// with the base state, which is the unbound local contract.
    // The receiver is kept although the body needs only the state: the bound entry
    // point is callable the same way as `handle_frame`, so the HTTP edge and every
    // other caller read alike and the choice of state stays the caller's only
    // visible difference.
    #[allow(clippy::unused_self)]
    pub(crate) fn handle_frame_with_state(&self, state: &McpState, frame: &str) -> Option<Value> {
        match serde_json::from_str::<Value>(frame) {
            Ok(msg) => Self::handle_message_with_state(state, &msg),
            Err(_) => Some(Self::error_response(
                None,
                &RpcError::new(ERR_PARSE, "Parse error: invalid JSON"),
            )),
        }
    }

    fn route(state: &McpState, msg: &Value) -> Result<Value, RpcError> {
        let method = msg
            .get("method")
            .and_then(serde_json::Value::as_str)
            .ok_or(invalid_request("method is required"))?;
        match method {
            "initialize" => Ok(Self::initialize(msg)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(Self::tools_list()),
            "tools/call" => Self::tools_call(state, msg.get("params")),
            other => Err(RpcError::new(
                ERR_METHOD_NOT_FOUND,
                format!("method `{other}` not found"),
            )),
        }
    }

    fn initialize(msg: &Value) -> Value {
        let requested = msg
            .pointer("/params/protocolVersion")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !requested.is_empty() && requested != MCP_PROTOCOL_REVISION {
            // Per the pinned-revision rule we answer with our own revision; a
            // client requiring a different revision aborts the handshake.
            eprintln!(
                "mcp: client requested revision `{requested}`, serving pin `{MCP_PROTOCOL_REVISION}`"
            );
        }
        json!({
            "protocolVersion": MCP_PROTOCOL_REVISION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": env!("CARGO_PKG_NAME"),
                "version": env!("CARGO_PKG_VERSION")
            }
        })
    }

    /// `tools/list` over the bound write + read sets (the same dispatcher serves
    /// both; their schemas come from the published pack).
    #[must_use]
    pub fn tools_list() -> Value {
        let bound = bound_set();
        let tools: Vec<Value> = catalog_defs()
            .iter()
            .filter(|def| bound.contains(&def.name))
            .map(crate::schema_defs::ToolDef::to_tool)
            .collect();
        json!({ "tools": tools })
    }

    fn tools_call(state: &McpState, params: Option<&Value>) -> Result<Value, RpcError> {
        let params = params.ok_or_else(|| invalid_params("tools/call requires params"))?;
        let name = params
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_params("tools/call.name is required"))?;
        if !bound_set().contains(&name) {
            return Err(invalid_params(format!("unknown tool `{name}`")));
        }
        let empty = json!({});
        let args = params.get("arguments").unwrap_or(&empty);
        // An argument the schema does not declare, or a blank required value, is
        // refused here. Before this check it was dropped, and the write went on.
        let result = match crate::arg_contract::check(name, args) {
            Ok(()) => crate::write_tools::dispatch(state, name, args),
            Err(err) => crate::write_tools::tool_result(Err(err)),
        };
        if let Some(err) = coactivation_rpc_error(&result) {
            // A failed durable handoff fails the call closed with a
            // retryable, server-defined JSON-RPC error.
            return Err(err);
        }
        if result.get("ok").and_then(Value::as_bool) == Some(true) {
            let bank = args
                .get("bank")
                .and_then(Value::as_str)
                .unwrap_or(&state.default_bank);
            if state.require_known_bank(bank).is_ok() {
                state.maintenance_trigger.watch_bank(bank);
            }
        }
        let is_error = result.get("ok").and_then(serde_json::Value::as_bool) != Some(true);
        let text = serde_json::to_string(&result).unwrap_or_else(|_| "{}".into());
        // Structured rejections keep their machine-readable code inside the
        // content payload and surface as `isError` tool results.
        Ok(json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
            "structuredContent": result
        }))
    }

    fn ok_response(id: Option<&Value>, result: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id.cloned().unwrap_or(Value::Null),
            "result": result
        })
    }

    fn error_response(id: Option<&Value>, err: &RpcError) -> Value {
        let mut error = json!({ "code": err.code, "message": err.message });
        if let Some(data) = &err.data {
            error["data"] = data.clone();
        }
        json!({
            "jsonrpc": "2.0",
            "id": id.cloned().unwrap_or(Value::Null),
            "error": error
        })
    }
}

/// A JSON-RPC error with an optional structured `data` payload.
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

fn invalid_request(message: &str) -> RpcError {
    RpcError::new(ERR_INVALID_REQUEST, message)
}

fn invalid_params(message: impl Into<String>) -> RpcError {
    RpcError::new(ERR_INVALID_PARAMS, message)
}

/// Map an co-activation handoff failure to the `-32001` RPC error.
///
/// Returns `None` for any other (or successful) tool result.
fn coactivation_rpc_error(result: &Value) -> Option<RpcError> {
    if result.get("code").and_then(Value::as_str) != Some("coactivation_handoff_failed") {
        return None;
    }
    Some(
        RpcError::new(
            ERR_COACTIVATION_HANDOFF,
            "Co-activation durability handoff failed",
        )
        .with_data(json!({
            "error": "COACTIVATION_HANDOFF_FAILED",
            "retryable": true,
            "query_executed": false
        })),
    )
}

/// The bound write + read set as a lookup slice.
#[must_use]
fn bound_set() -> Vec<&'static str> {
    crate::bound_tools()
}
#[cfg(test)]
#[path = "protocol_nfr7_tests.rs"]
mod protocol_nfr7_tests;
#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
