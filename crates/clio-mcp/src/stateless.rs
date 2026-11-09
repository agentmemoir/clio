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
//! - The refusal of a tool call that answers input this server never requested
//! - The `server/discover` result and the result fields the revision requires
//! - The one cache-hint pair every cacheable result of this dispatcher carries
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

use crate::conditional_http::{ERR_HEADER_MISMATCH, Refusal, check_headers, refusal};
use crate::protocol::{
    ERR_INTERNAL, ERR_INVALID_PARAMS, ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, ERR_PARSE,
    McpHandler, RpcError, invalid_params,
};
use crate::runtime::McpState;
use serde_json::{Map, Value, json};

/// The revision this dispatcher answers as.
pub(crate) const CANDIDATE_REVISION: &str = "2026-07-28";

/// Wire name of the header carrying the revision the client is using.
///
/// The transport lowercases header keys, so this is the key the getter sees.
pub(crate) const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

/// The header's name as a refusal message spells it.
const PROTOCOL_VERSION_HEADER_NAME: &str = "MCP-Protocol-Version";

/// Revisions this dispatcher accepts.
pub(crate) const SUPPORTED_REVISIONS: &[&str] = &[CANDIDATE_REVISION];

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

/// Request parameters key carrying state from a server input-required result.
const REQUEST_STATE: &str = "requestState";

/// Member that only an elicitation answer requires (`ElicitResult.action`).
const ELICITATION_ACTION: &str = "action";

/// Requests the served revision removed, so they answer as unknown methods.
const REMOVED_METHODS: &[&str] = &["initialize", "ping", "logging/setLevel"];

/// The cache hints a complete result of a cacheable request carries.
///
/// The revision requires both members on every such result and requires every
/// page of a paginated listing to share one scope, so a request answered in
/// several pages cannot pick a scope of its own.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CacheHints {
    /// How long, in milliseconds, a client may reuse the result as it stands.
    pub(crate) ttl_ms: i64,
    /// The scope the result may be reused in.
    pub(crate) scope: &'static str,
}

/// The one cache-hint pair this dispatcher answers for every cacheable result.
///
/// The two cacheable operations, discovery and the visible catalogue, share
/// this one freshness window and this one cache scope, so a client reads the
/// same reuse terms whichever of them it asks for, and no result may be handed
/// to a cache shared across authorization contexts.
pub(crate) const CACHE_HINTS: CacheHints = CacheHints {
    ttl_ms: 300_000,
    scope: "private",
};

/// Admit one candidate-revision HTTP request and, when it passes, serve it.
///
/// The checks run in one fixed order and stop at the first failure: the
/// conditional headers, JSON syntax, the required version header, the required
/// metadata, header/body equality, and the supported revision. Only a request
/// that passes every check reaches [`dispatch`].
///
/// `get` reads one header value by its lowercase wire name. A refused request
/// answers the status and JSON-RPC body [`refusal`] renders; a notification
/// that passes answers `None`, like [`dispatch`].
///
/// # Errors
/// The request fails one admission check. The refusal names the header that
/// refuses it, or the metadata key it lacks.
pub(crate) fn admit<'a, F>(state: &McpState, get: F, frame: &str) -> Result<Option<Value>, Refusal>
where
    F: Fn(&str) -> Option<&'a str>,
{
    let parsed = serde_json::from_str::<Value>(frame);
    // The guard checks a body it can read; an unreadable frame belongs to the
    // syntax step, which owns the parse error.
    if let Ok(body) = &parsed
        && let Err(err) = check_headers(&get, body)
    {
        return Err(refusal(body, &err));
    }
    let Ok(body) = parsed else {
        let err = RpcError::new(ERR_PARSE, "Parse error: invalid JSON");
        return Err(refusal(&Value::Null, &err));
    };
    let declared = get(PROTOCOL_VERSION_HEADER)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| missing_version_header(&body))?;
    let (_, requested) =
        request_meta(&body).map_err(|message| refusal(&body, &invalid_params(message)))?;
    if declared != requested {
        let err = RpcError::new(
            ERR_HEADER_MISMATCH,
            format!(
                "the `{PROTOCOL_VERSION_HEADER_NAME}` header `{declared}` is not the request metadata version `{requested}`"
            ),
        );
        return Err(refusal(&body, &err));
    }
    if !SUPPORTED_REVISIONS.contains(&requested) {
        let data = json!({ "supported": SUPPORTED_REVISIONS, "requested": requested });
        let err = RpcError::new(
            ERR_UNSUPPORTED_PROTOCOL_VERSION,
            format!("Unsupported protocol version in `{PROTOCOL_VERSION_HEADER_NAME}`"),
        )
        .with_data(data);
        return Err(refusal(&body, &err));
    }
    Ok(dispatch(state, &body))
}

/// The refusal for a request whose version header is absent or empty.
fn missing_version_header(body: &Value) -> Refusal {
    let err = RpcError::new(
        ERR_HEADER_MISMATCH,
        format!("the `{PROTOCOL_VERSION_HEADER_NAME}` header is required"),
    );
    refusal(body, &err)
}

/// Answer one candidate-revision message; `None` for a message without an `id`.
///
/// A message without an `id` is a notification, including the removed
/// `notifications/initialized`, and MCP never answers a notification.
#[must_use]
pub(crate) fn dispatch(state: &McpState, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?;
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(reject(id, ERR_INVALID_REQUEST, "method is required", None));
    };
    // A removed method is answered before any metadata rule: it is unknown
    // whether or not the request carries `_meta`.
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
    let params = msg.get("params");
    if let Some(capability) = required_capability(method, params)
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
        "tools/list" => match tools_list(state, params) {
            Ok(result) => ok(id, &result),
            Err(err) => reject(id, err.code, err.message, err.data),
        },
        // A real capability refusal has already answered, so a call that still
        // carries answers is refused as invalid params rather than served while
        // its answers are dropped.
        "tools/call" if params.is_some_and(answers_pending_input) => reject(
            id,
            ERR_INVALID_PARAMS,
            "Invalid params: this server has no pending input request to answer",
            None,
        ),
        "tools/call" => delegate(id, state, msg),
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
/// Only a `tools/call` that answers an elicitation input request needs one. In
/// this revision `ElicitResult` is the sole input answer that requires an
/// `action` member, so an answer carrying `action` is the only one read as an
/// elicitation answer; sampling and roots answers demand no capability. The
/// answers must be the object map the revision types them as: a shape that is
/// not an object is refused as invalid params, not turned into a capability
/// demand here.
#[must_use]
pub(crate) fn required_capability(method: &str, params: Option<&Value>) -> Option<&'static str> {
    let answers_elicitation = params
        .and_then(|params| params.get(INPUT_RESPONSES))
        .and_then(Value::as_object)
        .is_some_and(|answers| {
            answers
                .values()
                .any(|answer| answer.get(ELICITATION_ACTION).is_some())
        });
    match method {
        "tools/call" if answers_elicitation => Some("elicitation"),
        _ => None,
    }
}

/// Whether a tool call tries to answer input this server never requested.
///
/// The revision types `inputResponses` as an object map and `requestState` as a
/// string. A non-empty map, a non-string state, or a value of another shape all
/// claim a pending input request, and no such request is ever issued here.
fn answers_pending_input(params: &Value) -> bool {
    let answers = match params.get(INPUT_RESPONSES) {
        None | Some(Value::Null) => false,
        Some(Value::Object(entries)) => !entries.is_empty(),
        Some(_) => true,
    };
    answers
        || params
            .get(REQUEST_STATE)
            .is_some_and(|state| !state.is_null())
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
fn delegate(id: &Value, state: &McpState, msg: &Value) -> Value {
    let Some(mut response) = McpHandler::handle_message_with_state(state, msg) else {
        return reject(id, ERR_INTERNAL, "the tool call produced no response", None);
    };
    if let Some(result) = response.get_mut("result") {
        stamp_result(result);
    }
    response
}

/// Stamp the result fields the revision requires, keeping metadata already there.
fn stamp_result(result: &mut Value) {
    result["resultType"] = json!("complete");
    let Value::Object(identity) = result_meta() else {
        return;
    };
    match result.get_mut("_meta") {
        Some(Value::Object(existing)) => existing.extend(identity),
        _ => result["_meta"] = Value::Object(identity),
    }
}

/// The `server/discover` result: versions, capabilities, and identity.
#[must_use]
fn discover() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": SUPPORTED_REVISIONS,
        "capabilities": { "tools": { "listChanged": false } },
        "ttlMs": CACHE_HINTS.ttl_ms,
        "cacheScope": CACHE_HINTS.scope,
        "_meta": result_meta()
    })
}

/// The visible catalogue page, stamped with the result fields the revision
/// requires.
///
/// The catalogue itself comes from the production handler, so tool definitions,
/// the configured profile, and paging keep one source. The cache hints are the
/// module's one decision value, so a page never promises what discovery does
/// not.
fn tools_list(state: &McpState, params: Option<&Value>) -> Result<Value, RpcError> {
    let mut result = McpHandler::tools_list_page(state, params)?;
    stamp_result(&mut result);
    result["ttlMs"] = json!(CACHE_HINTS.ttl_ms);
    result["cacheScope"] = json!(CACHE_HINTS.scope);
    Ok(result)
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

#[cfg(test)]
#[path = "stateless_cache_tests.rs"]
mod stateless_cache_tests;

#[cfg(test)]
#[path = "version_header_tests.rs"]
mod version_header_tests;
