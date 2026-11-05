// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for a non-activated candidate dispatcher that
//! answers MCP JSON-RPC requests under the rules of revision `2026-07-28`:
//! per-request metadata, removed methods, and that revision's error codes.
//!
//! ## Owns
//! - The candidate revision constant and the supported-revision list
//! - `params._meta` validation, the unsupported-version error, and the
//!   required-client-capability refusal
//! - The `server/discover` result and the result fields the revision requires
//!
//! ## Does not own
//! - The pinned revision, the tool catalogue, and the transports (see
//!   `protocol`, `http`, `stdio`)
//! - Tool semantics (see `write_tools`, `read_tools`)
//! - Activation: only this crate's tests compile and reach this module
//!
//! ## Boundary
//!
//! Changes to the candidate revision's request rules should normally require
//! changes here. Changes to the pinned revision, a transport, or a tool should
//! not require changes here.
//!
//! Keep this module focused on the candidate revision's request handling.

use crate::protocol::{ERR_INVALID_PARAMS, ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, McpHandler};
use crate::runtime::McpState;
use serde_json::{Map, Value, json};

/// The revision this dispatcher answers as.
pub const CANDIDATE_REVISION: &str = "2026-07-28";

/// Revisions this dispatcher accepts.
pub const SUPPORTED_REVISIONS: &[&str] = &[CANDIDATE_REVISION];

/// JSON-RPC unsupported protocol version.
const ERR_UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// JSON-RPC missing required client capability.
const ERR_MISSING_REQUIRED_CLIENT_CAPABILITY: i64 = -32021;

/// Request metadata key carrying the revision the client is using.
const PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";

/// Request metadata key carrying the client's per-request capabilities.
const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

/// Request parameters key carrying answers to the server's input requests.
const INPUT_RESPONSES: &str = "inputResponses";

/// Requests the served revision removed, so they answer as unknown methods.
const REMOVED_METHODS: &[&str] = &["initialize", "ping", "logging/setLevel"];

/// The client capability each request shape needs, keyed by method.
const REQUIRED_CAPABILITIES: &[(&str, &str)] = &[("tools/call", ELICITATION)];

/// The client capability an input request is answered with.
const ELICITATION: &str = "elicitation";

/// Cache hint for discovery, whose content is fixed for a given build.
const DISCOVER_CACHE_TTL_MS: i64 = 3_600_000;

/// Answer one candidate-revision message; `None` for a message without an `id`.
///
/// A message without an `id` is a notification, including the removed
/// `notifications/initialized`, and MCP never answers a notification.
#[must_use]
pub fn dispatch(state: &McpState, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?;
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(reject(id, ERR_INVALID_REQUEST, "method is required", None));
    };
    if REMOVED_METHODS.contains(&method) {
        return Some(reject(
            id,
            ERR_METHOD_NOT_FOUND,
            format!("method `{method}` not found"),
            None,
        ));
    }
    let (capabilities, requested) = match request_meta(msg) {
        Ok(meta) => meta,
        Err(message) => return Some(reject(id, ERR_INVALID_PARAMS, message, None)),
    };
    if !SUPPORTED_REVISIONS.contains(&requested) {
        let data = json!({ "supported": SUPPORTED_REVISIONS, "requested": requested });
        return Some(reject(
            id,
            ERR_UNSUPPORTED_PROTOCOL_VERSION,
            "Unsupported protocol version",
            Some(data),
        ));
    }
    if let Some(capability) = required_capability(method, msg.get("params"))
        && !capabilities.contains_key(capability)
    {
        let data = json!({ "requiredCapabilities": { (capability): {} } });
        return Some(reject(
            id,
            ERR_MISSING_REQUIRED_CLIENT_CAPABILITY,
            format!("request requires the client capability `{capability}`"),
            Some(data),
        ));
    }
    Some(match method {
        "server/discover" => ok(id, &discover()),
        "tools/list" => ok(id, &tools_list()),
        "tools/call" => delegate(state, msg),
        other => reject(
            id,
            ERR_METHOD_NOT_FOUND,
            format!("method `{other}` not found"),
            None,
        ),
    })
}

/// The client capability a request needs from its declared capabilities.
///
/// Only a request that answers the server's input requests needs one.
#[must_use]
pub fn required_capability(method: &str, params: Option<&Value>) -> Option<&'static str> {
    let capability = REQUIRED_CAPABILITIES
        .iter()
        .find(|(name, _)| *name == method)
        .map(|(_, capability)| *capability)?;
    let answers = params
        .and_then(|params| params.get(INPUT_RESPONSES))
        .is_some_and(|responses| match responses {
            Value::Array(entries) => !entries.is_empty(),
            Value::Object(entries) => !entries.is_empty(),
            _ => false,
        });
    answers.then_some(capability)
}

/// The client's declared capabilities and the revision it requested.
fn request_meta(msg: &Value) -> Result<(&Map<String, Value>, &str), String> {
    let meta = msg
        .pointer("/params/_meta")
        .and_then(Value::as_object)
        .ok_or_else(|| "Invalid params: `params._meta` is required as an object".to_string())?;
    let requested = meta
        .get(PROTOCOL_VERSION)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Invalid params: `{PROTOCOL_VERSION}` is required as a string"))?;
    let capabilities = meta
        .get(CLIENT_CAPABILITIES)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!("Invalid params: `{CLIENT_CAPABILITIES}` is required as an object")
        })?;
    Ok((capabilities, requested))
}

/// Serve a tool call from the production message path, stamped for the revision.
fn delegate(state: &McpState, msg: &Value) -> Value {
    let mut response = McpHandler::handle_message_with_state(state, msg)
        .expect("the dispatcher has already accepted this message's id");
    if let Some(result) = response.get_mut("result") {
        result["resultType"] = json!("complete");
        result["_meta"] = result_meta();
    }
    response
}

/// The `server/discover` result: versions, capabilities, and identity.
#[must_use]
fn discover() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": SUPPORTED_REVISIONS,
        "capabilities": { "tools": { "listChanged": false } },
        "ttlMs": DISCOVER_CACHE_TTL_MS,
        "cacheScope": "public",
        "_meta": result_meta()
    })
}

/// The bound catalogue, stamped with the result fields the revision requires.
///
/// The catalogue itself comes from the production handler, so tool definitions
/// keep one source. A zero TTL promises no freshness; the private scope keeps
/// the list out of any cache shared across authorization contexts.
#[must_use]
fn tools_list() -> Value {
    let mut result = McpHandler::tools_list();
    result["resultType"] = json!("complete");
    result["ttlMs"] = json!(0);
    result["cacheScope"] = json!("private");
    result["_meta"] = result_meta();
    result
}

/// The result metadata block carrying the server's identity.
#[must_use]
fn result_meta() -> Value {
    json!({ "io.modelcontextprotocol/serverInfo": server_info() })
}

/// Name and version of this server, from the crate's own package metadata.
#[must_use]
fn server_info() -> Value {
    json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION")
    })
}

/// A successful JSON-RPC response envelope.
#[must_use]
fn ok(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A JSON-RPC error envelope, with `data` only when the error carries it.
#[must_use]
fn reject(id: &Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message.into() });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

#[cfg(test)]
#[path = "stateless_tests.rs"]
mod stateless_tests;
