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
//! - The bound write+read tool set that dispatch and the argument contract read
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
use crate::discovery_tools::TOOL_DESCRIBE;
use crate::runtime::McpState;
use crate::schema::catalog_defs;
use crate::tools_page::ToolView;
use clio_types::tools_of;
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

    pub(crate) fn handle_message_with_state(state: &McpState, msg: &Value) -> Option<Value> {
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
            "tools/list" => Self::tools_list_page(state, msg.get("params")),
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

    /// The full bound write + read set, which dispatch and the argument
    /// contract read; `tools/list` advertises the profile-narrowed subset.
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

    /// `tools/list` over the bound catalogue narrowed to the groups the effective
    /// selection names.
    ///
    /// Only what this build advertises shrinks: dispatch eligibility and the
    /// argument contract keep reading [`Self::tools_list`], so a tool outside
    /// the selected groups stays reachable and stays validated. The stateless
    /// transport answers through [`Self::tools_list_page`] too, so one path
    /// decides what every transport advertises.
    ///
    /// # Errors
    /// The effective configuration names a tool profile this build does not
    /// know and no installed selection governs the view.
    #[cfg(test)]
    pub(crate) fn tools_list_visible(state: &McpState) -> Result<Value, RpcError> {
        let (_, tools) = visible_view(state)?;
        Ok(json!({ "tools": tools }))
    }

    /// `tools/list` with the page the request selects.
    ///
    /// The view is the same whole listing the profile advertises; only the
    /// slice and its `nextCursor` differ. A request that selects no page size
    /// gets the default page.
    ///
    /// # Errors
    /// The effective configuration names an unknown profile and no installed
    /// selection governs the view, or the request carries a page size or cursor
    /// this build refuses.
    pub(crate) fn tools_list_page(
        state: &McpState,
        params: Option<&Value>,
    ) -> Result<Value, RpcError> {
        let (view, tools) = visible_view(state)?;
        crate::tools_page::page(&tools, &view, params)
    }

    fn tools_call(state: &McpState, params: Option<&Value>) -> Result<Value, RpcError> {
        let params = params.ok_or_else(|| invalid_params("tools/call requires params"))?;
        let name = params
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_params("tools/call.name is required"))?;
        let empty = json!({});
        let args = params.get("arguments").unwrap_or(&empty);
        // A bound tool that later takes this name keeps the normal dispatch
        // path, so the guard names binding explicitly instead of assuming the
        // discovery tool is never bound.
        if name == TOOL_DESCRIBE && !bound_set().contains(&name) {
            let payload = crate::discovery_tools::describe(args)?;
            return Ok(json!({
                "content": [{ "type": "text", "text": "ok" }],
                "isError": false,
                "structuredContent": payload
            }));
        }
        // `tool_search` dispatches over the whole catalog, so it serves a name
        // outside the bound set while the discovery setting keeps it on.
        if name == crate::catalog_search::NAME {
            if !crate::catalog_search::enabled(state) {
                return Err(unknown_tool(name));
            }
            let page = crate::catalog_search::call(args)?;
            return Ok(crate::catalog_search::envelope(page));
        }
        if !bound_set().contains(&name) {
            return Err(unknown_tool(name));
        }
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
        let has_schema = crate::schema_output_defs::output_schema(name).is_some();
        Ok(tool_call_envelope(&result, has_schema))
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
#[derive(Debug)]
pub(crate) struct RpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
    pub(crate) data: Option<Value>,
}

impl RpcError {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
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

pub(crate) fn invalid_params(message: impl Into<String>) -> RpcError {
    RpcError::new(ERR_INVALID_PARAMS, message)
}

/// The one refusal for a tool name this server does not dispatch.
pub(crate) fn unknown_tool(name: &str) -> RpcError {
    invalid_params(format!("unknown tool `{name}`"))
}

/// Build a `tools/call` result envelope.
///
/// A tool with a declared output schema carries the payload once, in
/// `structuredContent`; its `content` is a short status line. A tool with no
/// declared output schema carries the serialized payload in `content` and no
/// `structuredContent` key, because that key only means something when a
/// schema describes its shape.
///
/// A client that reads only the text block of a schema-declaring result gets a
/// status line (`ok`) on success, and the error code and message on failure.
/// It does not get the payload.
///
/// The protocol recommendation for backwards compatibility is to also send the
/// serialized payload as a JSON text block. This envelope omits that copy on
/// purpose, so a payload is never sent twice. A content-only client gives up
/// the result: it sees only the status line, or the error code and message.
pub(crate) fn tool_call_envelope(result: &Value, has_schema: bool) -> Value {
    let is_error = result.get("ok").and_then(Value::as_bool) != Some(true);
    if has_schema {
        let text = if is_error {
            error_summary(result)
        } else {
            "ok".to_owned()
        };
        json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
            "structuredContent": result
        })
    } else {
        let text = serde_json::to_string(result).unwrap_or_else(|_| "{}".into());
        json!({
            "content": [{ "type": "text", "text": text }],
            "isError": is_error
        })
    }
}

/// A short error line built from the error fields the payload actually holds.
fn error_summary(result: &Value) -> String {
    let code = result.get("code").and_then(Value::as_str);
    let message = result.get("message").and_then(Value::as_str);
    match (code, message) {
        (Some(code), Some(message)) => format!("error: {code}: {message}"),
        (Some(code), None) => format!("error: {code}"),
        (None, Some(message)) => format!("error: {message}"),
        (None, None) => "error".to_owned(),
    }
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

/// The selected profile, the effective group selection, and the visible
/// definitions it advertises, in catalog order, read under one configuration
/// lock.
///
/// The groups come from the effective selection, so an installed `--tools`
/// selection wins over `mcp.tool_profile`. When a selection governs the view,
/// `mcp_tool_groups` answers without the document, so a missing or unknown
/// document preset cannot fail the request; the profile is then absent from the
/// snapshot.
fn visible_view(state: &McpState) -> Result<(ToolView, Vec<Value>), RpcError> {
    let rt: std::sync::MutexGuard<'_, clio_config::Runtime> =
        state.config.lock().expect("config runtime mutex poisoned");
    let groups = rt.mcp_tool_groups();
    let preset = rt.mcp_tool_profile();
    let discovery = rt.mcp_discovery();
    let revision = rt
        .effective_raw()
        .pointer("/mcp/revision")
        .and_then(Value::as_str)
        .map(str::to_owned);
    drop(rt);
    let groups = groups.map_err(|err| RpcError::new(ERR_INTERNAL, err.message))?;
    let visible = tools_of(&groups);
    let bound = bound_set();
    let mut tools: Vec<Value> = catalog_defs()
        .iter()
        .filter(|def| bound.contains(&def.name) && visible.contains(&def.name))
        .map(crate::schema_defs::ToolDef::to_tool)
        .collect();
    if discovery {
        tools.push(crate::catalog_search::listing_entry());
    }
    let view = ToolView {
        preset: preset.ok(),
        groups,
        revision: revision.unwrap_or_else(|| MCP_PROTOCOL_REVISION.to_owned()),
    };
    Ok((view, tools))
}

#[cfg(test)]
#[path = "protocol_nfr7_tests.rs"]
mod protocol_nfr7_tests;
#[cfg(test)]
#[path = "protocol_output_schema_tests.rs"]
mod protocol_output_schema_tests;

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;

#[cfg(test)]
#[path = "tool_profile_tests.rs"]
mod tool_profile_tests;

#[cfg(test)]
#[path = "tool_profile_parity_tests.rs"]
mod tool_profile_parity_tests;
