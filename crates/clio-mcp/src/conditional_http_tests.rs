// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit fixtures of the candidate
//! conditional-header guard: the required method header, the conditional name
//! header, and the sentinel decode of an encoded name.
//!
//! ## Owns
//! - Every assertion about `check_headers`, `decode_name_header`, and the
//!   header constants
//!
//! ## Does not own
//! - The guard itself (`conditional_http`)
//! - The HTTP route and its response status (see the crate's integration
//!   suites)
//!
//! ## Boundary
//!
//! Changes to the candidate guard's header rules should normally require
//! changes here. Changes to the pinned revision's suites should not.
//!
//! Keep this module focused on the candidate guard's fixtures.

use super::{
    ERR_HEADER_MISMATCH, MCP_METHOD_HEADER, MCP_NAME_HEADER, NAME_REQUIRING_METHODS,
    SENTINEL_PREFIX, SENTINEL_SUFFIX, check_headers, decode_name_header,
};
use crate::protocol::RpcError;
use base64::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;

/// The sentinel form of one name, as a client sends it.
fn encoded(name: &str) -> String {
    format!(
        "{SENTINEL_PREFIX}{}{SENTINEL_SUFFIX}",
        base64::engine::general_purpose::STANDARD.encode(name.as_bytes())
    )
}

/// Run the guard over one request body with the given headers.
fn check(headers: &[(&str, &str)], body: &Value) -> Result<(), RpcError> {
    let supplied: HashMap<&str, &str> = headers.iter().copied().collect();
    check_headers(|name: &str| supplied.get(name).copied(), body)
}

/// Run the guard over a request it must refuse, and answer that refusal.
fn fault(headers: &[(&str, &str)], body: &Value) -> RpcError {
    check(headers, body).expect_err("the guard refuses this request")
}

/// A `tools/call` body naming one tool.
fn tool_call(name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": {} }
    })
}

/// The refusal code and the header it names, asserted together.
fn assert_fault(headers: &[(&str, &str)], body: &Value, header: &str) -> String {
    let err = fault(headers, body);
    assert_eq!(err.code, ERR_HEADER_MISMATCH, "{err:?}");
    assert_eq!(err.code, -32020, "{err:?}");
    assert!(
        err.message.to_lowercase().contains(header),
        "names `{header}`: {}",
        err.message
    );
    err.message
}

#[test]
fn only_tools_call_requires_a_name() {
    assert_eq!(NAME_REQUIRING_METHODS.len(), 1);
    assert_eq!(
        NAME_REQUIRING_METHODS[0],
        ("tools/call", "/params/name"),
        "the table ties the method to the body field its name must equal"
    );
}

#[test]
fn an_encoded_name_matching_the_body_passes() {
    let body = tool_call("clio_read");
    let name = encoded("clio_read");
    check(
        &[(MCP_METHOD_HEADER, "tools/call"), (MCP_NAME_HEADER, &name)],
        &body,
    )
    .expect("the header and the body name agree");
}

#[test]
fn a_sentinel_name_of_a_non_ascii_tool_is_decoded_before_comparing() {
    let name = encoded("工具/读");
    let body = tool_call("工具/读");
    check(
        &[(MCP_METHOD_HEADER, "tools/call"), (MCP_NAME_HEADER, &name)],
        &body,
    )
    .expect("the decode is over UTF-8 bytes, not ASCII");
}

#[test]
fn a_sentinel_name_is_literally_the_base64_of_the_body_name() {
    let body = tool_call("clio_read");
    check(
        &[
            (MCP_METHOD_HEADER, "tools/call"),
            (MCP_NAME_HEADER, "=?base64?Y2xpb19yZWFk?="),
        ],
        &body,
    )
    .expect("canonical standard base64 of the name");
}

#[test]
fn a_plain_name_matching_the_body_passes() {
    check(
        &[
            (MCP_METHOD_HEADER, "tools/call"),
            (MCP_NAME_HEADER, "clio_read"),
        ],
        &tool_call("clio_read"),
    )
    .expect("a name outside the sentinel is compared as it stands");
}

#[test]
fn a_missing_method_header_is_refused() {
    let message = assert_fault(
        &[(MCP_NAME_HEADER, "clio_read")],
        &tool_call("clio_read"),
        MCP_METHOD_HEADER,
    );
    assert!(message.contains("required"), "{message}");
}

#[test]
fn a_method_header_that_is_not_the_body_method_is_refused() {
    let body = tool_call("clio_read");
    for declared in ["tools/list", "Tools/Call", "tools/call ", ""] {
        assert_fault(
            &[
                (MCP_METHOD_HEADER, declared),
                (MCP_NAME_HEADER, "clio_read"),
            ],
            &body,
            MCP_METHOD_HEADER,
        );
    }
}

#[test]
fn a_missing_name_header_on_a_tool_call_is_refused() {
    assert_fault(
        &[(MCP_METHOD_HEADER, "tools/call")],
        &tool_call("clio_read"),
        MCP_NAME_HEADER,
    );
}

#[test]
fn a_malformed_encoded_name_is_refused() {
    let body = tool_call("clio_read");
    for raw in [
        // The prefix is never closed.
        "=?base64?Y2xpb19yZWFk",
        // The prefix is closed by the wrong suffix.
        "=?base64?Y2xpb19yZWFk?",
        // The payload is not Base64 at all.
        "=?base64?clio_read!?=",
        // The payload carries a padding the canonical form does not.
        "=?base64?Y2xpb19yZWFk=?=",
        // The payload decodes to bytes that are not UTF-8.
        "=?base64?//8=?=",
    ] {
        assert_fault(
            &[(MCP_METHOD_HEADER, "tools/call"), (MCP_NAME_HEADER, raw)],
            &body,
            MCP_NAME_HEADER,
        );
    }
}

#[test]
fn an_encoded_name_that_decodes_to_another_name_is_refused() {
    let name = encoded("clio_read");
    let message = assert_fault(
        &[(MCP_METHOD_HEADER, "tools/call"), (MCP_NAME_HEADER, &name)],
        &tool_call("clio_write"),
        MCP_NAME_HEADER,
    );
    assert!(message.contains("clio_read"), "{message}");
    assert!(message.contains("clio_write"), "{message}");
}

#[test]
fn a_plain_value_outside_the_sentinel_is_compared_and_can_be_refused() {
    let body = tool_call("clio_read");
    for raw in [
        "clio_write",
        "=?BASE64?Y2xpb19yZWFk?=",
        "= ?base64?Y2xpb19yZWFk?=",
    ] {
        assert_fault(
            &[(MCP_METHOD_HEADER, "tools/call"), (MCP_NAME_HEADER, raw)],
            &body,
            MCP_NAME_HEADER,
        );
    }
}

#[test]
fn methods_that_do_not_name_a_resource_need_no_name_header() {
    for method in [
        "tools/list",
        "server/discover",
        "subscriptions/listen",
        "ping",
        "resources/list",
    ] {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": {} });
        check(&[(MCP_METHOD_HEADER, method)], &body).expect("only the method header is required");
    }
}

#[test]
fn a_name_header_on_a_method_that_needs_none_is_ignored() {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    check(
        &[
            (MCP_METHOD_HEADER, "tools/list"),
            (MCP_NAME_HEADER, "=?base64??="),
        ],
        &body,
    )
    .expect("a listing carries no name to compare");
}

#[test]
fn a_body_with_no_comparable_method_or_name_is_left_to_the_json_rpc_layer() {
    for body in [
        json!({ "jsonrpc": "2.0", "id": 1 }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": 7 }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {} }),
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": 7 }
        }),
    ] {
        check(
            &[(MCP_METHOD_HEADER, "tools/call"), (MCP_NAME_HEADER, "x")],
            &body,
        )
        .expect("the guard adds no refusal the JSON-RPC layer owns");
    }
}

#[test]
fn a_plain_name_is_decoded_to_itself() {
    assert_eq!(decode_name_header("clio_read").expect("plain"), "clio_read");
    assert_eq!(decode_name_header("").expect("empty plain"), "");
}

#[test]
fn an_encoded_name_is_decoded_to_the_name_it_carries() {
    for name in ["clio_read", "工具/读", "=?base64?literal?="] {
        assert_eq!(
            decode_name_header(&encoded(name)).expect("encoded"),
            name,
            "one wrapping carries the literal name"
        );
    }
}

#[test]
fn an_unclosed_or_unreadable_sentinel_is_refused_by_the_decode() {
    for raw in [
        "=?base64?",
        "=?base64?Y2xpb19yZWFk",
        "=?base64?Y2xpb19yZWFk!?=",
    ] {
        let err = decode_name_header(raw).expect_err("malformed");
        assert_eq!(err.code, ERR_HEADER_MISMATCH, "{raw}: {err:?}");
    }
}
