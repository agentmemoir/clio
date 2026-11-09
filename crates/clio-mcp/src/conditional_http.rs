// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for a non-activated candidate guard that
//! validates the conditional request headers of revision `2026-07-28`:
//! `Mcp-Method` on every request, and `Mcp-Name` only for the methods that
//! name a resource, decoded from that revision's `Base64` sentinel form.
//!
//! ## Owns
//! - The header-mismatch error code and the two header wire names
//! - The sentinel markers and the decode of an encoded `Mcp-Name`
//! - The one table of body methods that require a name, with the body field
//!   each name is compared against
//! - The guard that compares the headers with one request body
//! - The refusal a route answers a failed guard with: the status and the
//!   JSON-RPC error body
//!
//! ## Does not own
//! - The HTTP route and its framing (see `http` and `http_read`); this module
//!   only names the status a request the guard refuses is answered with
//! - The pinned revision constant and the JSON-RPC envelope (see the crate
//!   root and `protocol`)
//! - Activation: only this crate's tests compile and reach this module
//!
//! ## Boundary
//!
//! Changes to the candidate revision's header rules should normally require
//! changes here. Changes to the route, the pinned revision, or a tool should
//! not require changes here.
//!
//! Keep this module focused on checking the candidate revision's request
//! headers against the body.

use crate::protocol::{RpcError, error_response};
use base64::Engine;
use serde_json::Value;

/// JSON-RPC header mismatch.
pub(crate) const ERR_HEADER_MISMATCH: i64 = -32020;

/// Wire name of the header carrying the request body's method.
pub(crate) const MCP_METHOD_HEADER: &str = "mcp-method";

/// Wire name of the header carrying the name of the resource a method
/// addresses.
pub(crate) const MCP_NAME_HEADER: &str = "mcp-name";

/// Opening marker of the sentinel that marks an encoded `Mcp-Name`.
pub(crate) const SENTINEL_PREFIX: &str = "=?base64?";

/// Closing marker of the sentinel that marks an encoded `Mcp-Name`.
pub(crate) const SENTINEL_SUFFIX: &str = "?=";

/// The body methods that require `Mcp-Name`, each with the body pointer its
/// name must equal.
///
/// Only a method this tree routes can be listed. The message router serves
/// `tools/call` and no other method that names a resource, so the named read
/// and prompt methods stay out of the table until they are routed.
pub(crate) const NAME_REQUIRING_METHODS: &[(&str, &str)] = &[("tools/call", "/params/name")];

/// Decode one `Mcp-Name` header value into the resource name it carries.
///
/// A value that does not start with the sentinel prefix is a plain name and
/// passes through unchanged; a value that starts with it must also end with
/// the suffix and hold canonical standard `Base64` of valid UTF-8, and is
/// otherwise malformed, never read as a plain name.
pub(crate) fn decode_name_header(value: &str) -> Result<String, RpcError> {
    let Some(rest) = value.strip_prefix(SENTINEL_PREFIX) else {
        return Ok(value.to_owned());
    };
    let encoded = rest
        .strip_suffix(SENTINEL_SUFFIX)
        .ok_or_else(malformed_name)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| malformed_name())?;
    String::from_utf8(bytes).map_err(|_| malformed_name())
}

/// The one refusal for an encoded `Mcp-Name` that cannot be read.
fn malformed_name() -> RpcError {
    RpcError::new(
        ERR_HEADER_MISMATCH,
        "the `Mcp-Name` header is not a well-formed encoded name",
    )
}

/// Compare the request headers with the body of one JSON-RPC request.
///
/// `get` reads one header value by its lowercase wire name; the value it
/// answers borrows the request that holds the headers, not the name it was
/// asked for.
///
/// # Errors
/// `Mcp-Method` is absent or is not the body's method, or `Mcp-Name` is absent
/// for a name-requiring method, malformed, or not the name the body carries;
/// every one of those is the header-mismatch code. A body with no string
/// `method`, or a name-requiring method whose body carries no string name, is
/// left to the JSON-RPC layer that refuses such a request.
pub(crate) fn check_headers<'a, F>(get: F, body: &Value) -> Result<(), RpcError>
where
    F: Fn(&str) -> Option<&'a str>,
{
    let Some(method) = body.get("method").and_then(Value::as_str) else {
        return Ok(());
    };
    let declared = get(MCP_METHOD_HEADER).ok_or_else(missing_method)?;
    if declared != method {
        return Err(RpcError::new(
            ERR_HEADER_MISMATCH,
            format!("the `Mcp-Method` header `{declared}` is not the request method `{method}`"),
        ));
    }
    let Some((_, field)) = NAME_REQUIRING_METHODS
        .iter()
        .find(|(named, _)| *named == method)
    else {
        return Ok(());
    };
    let raw = get(MCP_NAME_HEADER).ok_or_else(|| {
        RpcError::new(
            ERR_HEADER_MISMATCH,
            format!("the `Mcp-Name` header is required for `{method}`"),
        )
    })?;
    let name = decode_name_header(raw)?;
    let Some(body_name) = body.pointer(field).and_then(Value::as_str) else {
        return Ok(());
    };
    if name != body_name {
        return Err(RpcError::new(
            ERR_HEADER_MISMATCH,
            format!("the `Mcp-Name` header `{name}` is not the request name `{body_name}`"),
        ));
    }
    Ok(())
}

/// The one refusal for a request carrying no `Mcp-Method` header.
fn missing_method() -> RpcError {
    RpcError::new(ERR_HEADER_MISMATCH, "the `Mcp-Method` header is required")
}

/// The candidate refusal for one request: the status and the JSON-RPC body a
/// route answers when the guard fails.
///
/// `None` is a request whose headers agree with its body, which the route goes
/// on to serve. The body is [`error_response`]'s envelope over the
/// header-mismatch error, so it carries no `result`, and the request's own id,
/// or `null` when the body has none.
///
/// `get` reads one header value by its lowercase wire name, as in
/// [`check_headers`].
pub(crate) fn refuse<'a, F>(get: F, body: &Value) -> Option<(u16, String)>
where
    F: Fn(&str) -> Option<&'a str>,
{
    let err = check_headers(get, body).err()?;
    let envelope = error_response(body.get("id"), &err);
    Some((400, serde_json::to_string(&envelope).unwrap_or_default()))
}

#[cfg(test)]
#[path = "conditional_http_event_id_tests.rs"]
mod conditional_http_event_id_tests;
#[cfg(test)]
#[path = "conditional_http_tests.rs"]
mod conditional_http_tests;
