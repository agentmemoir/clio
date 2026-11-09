// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures of the candidate
//! `2026-07-28` dispatcher's unit tests: runtime state, request metadata, the
//! tool-call and answer shapes those fixtures send, and the result checkers the
//! transport test modules assert with.
//!
//! ## Owns
//! - The one candidate fixture revision pin every candidate test module sends
//! - The fixture runtime state, its registered bank, and the seeded domain-read
//!   state
//! - The request envelopes and `_meta` variants the candidate tests send
//! - The read tool-call builder and accessors for a response's error code,
//!   error message, and server identity
//! - The success, error, completeness, discovery, and domain-read result
//!   checkers both candidate transports assert with
//! - The advertised catalogue names and the check a multi-page listing walk
//!   answers to
//!
//! ## Does not own
//! - Any scenario or the choice of assertion (see `stateless_tests`,
//!   `stdio_candidate_tests`, `http_candidate_tests`)
//! - The dispatcher itself (see `stateless`)
//!
//! ## Boundary
//!
//! Changes to how the candidate tests build a request or check a result should
//! normally require changes here. Changes to what a fixture asserts should not.
//!
//! Keep this module free of test functions.

use serde_json::{Map, Value, json};
use std::sync::Arc;

use crate::runtime::{McpOpenOptions, McpState};
use crate::tools_page::PAGE_SIZE_KEY;

pub(super) const BANK: &str = "fixture-bank";

/// The revision the conformance scenarios send.
///
/// It is the crate's own pin rather than a restated literal, so a scenario can
/// never sit on a stale revision that still passes.
pub(crate) const CANDIDATE_FIXTURE_REVISION: &str = crate::MCP_PROTOCOL_REVISION;

/// The one stored item the domain-read fixtures seed: its id, the gist its read
/// carries, and the category and epistemic kind its listing row carries.
pub(super) const DOMAIN_ITEM_ID: &str = "fixture-item-1";
pub(super) const DOMAIN_GIST: &str = "the canonical fixture gist";
pub(super) const DOMAIN_CATEGORY: &str = "task_spec";
pub(super) const DOMAIN_EPISTEMIC_KIND: &str = "fact";
const DOMAIN_TS: &str = "2024-01-01T00:00:00Z";

pub(super) fn state() -> Arc<McpState> {
    let state =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    state
        .ops_store
        .register_bank(BANK, "2024-01-01T00:00:00Z")
        .expect("bank fixture");
    state
}

/// The fixture state with the one stored item a domain read needs.
///
/// The item is written through the production store tool, so the row a read
/// serves is the row a caller would have stored, not a hand-built value.
pub(super) fn domain_state() -> Arc<McpState> {
    let state = state();
    let stored = crate::write_tools::dispatch(
        &state,
        "store",
        &json!({
            "item": {
                "id": DOMAIN_ITEM_ID,
                "subject_id": "subj",
                "gist": DOMAIN_GIST,
                "snapshot": { "entity": "fixture", "amount": 1, "date": "2024-01-01" }
            },
            "category": DOMAIN_CATEGORY,
            "epistemic_kind": DOMAIN_EPISTEMIC_KIND,
            "source_text": "The fixture states the canonical gist (1 note since 2024-01-01).",
            "bank": BANK,
            "actor": "agent",
            "timestamp": DOMAIN_TS
        }),
    );
    assert_eq!(
        stored["ok"],
        json!(true),
        "seed the domain read item: {stored}"
    );
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

/// A read tool call: `tools/call` naming `name` with `arguments` on the fixture
/// bank. The bank is the fixture's, so a read never depends on the caller.
pub(super) fn read_call(meta: &Value, name: &str, mut arguments: Value) -> Value {
    if let Value::Object(fields) = &mut arguments {
        fields.insert("bank".into(), json!(BANK));
    }
    let mut params = Map::new();
    params.insert("_meta".into(), meta.clone());
    params.insert("name".into(), json!(name));
    params.insert("arguments".into(), arguments);
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": params })
}

/// A tool call this build cannot serve without a configured extraction
/// provider, so the tool answers a result whose `isError` is `true` rather than
/// a JSON-RPC error: the failure is the tool's, not the protocol's.
pub(super) fn failing_tool_call(meta: &Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {
            "_meta": meta,
            "name": "summarize",
            "arguments": { "scope": "bank" }
        }
    })
}

/// A `tools/list` request for one page of a view: the page size the extension
/// asks for, and the cursor of the page before it when the walk has started.
pub(super) fn paged_list(meta: &Value, size: usize, cursor: Option<&str>) -> Value {
    let mut params = Map::new();
    let mut meta = meta.clone();
    meta[PAGE_SIZE_KEY] = json!(size);
    params.insert("_meta".into(), meta);
    if let Some(cursor) = cursor {
        params.insert("cursor".into(), json!(cursor));
    }
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": params })
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

/// A success envelope: JSON-RPC 2.0, the request id, one result, and no error.
pub(super) fn check_success_envelope(envelope: &Value, id: i64) -> Result<(), String> {
    if envelope["jsonrpc"] != json!("2.0") {
        return Err(format!("not JSON-RPC 2.0: {envelope}"));
    }
    if envelope.get("result").is_none() {
        return Err(format!("a success envelope carries a result: {envelope}"));
    }
    if envelope.get("error").is_some() {
        return Err(format!("a success envelope carries no error: {envelope}"));
    }
    if envelope["id"] != json!(id) {
        return Err(format!("the envelope id is not `{id}`: {envelope}"));
    }
    Ok(())
}

/// An error envelope: JSON-RPC 2.0, the request id, one error, no result, and
/// no result type anywhere.
pub(super) fn check_error_envelope(envelope: &Value, id: i64) -> Result<(), String> {
    if envelope["jsonrpc"] != json!("2.0") {
        return Err(format!("not JSON-RPC 2.0: {envelope}"));
    }
    if envelope.get("error").is_none() {
        return Err(format!("an error envelope carries an error: {envelope}"));
    }
    if envelope.get("result").is_some() {
        return Err(format!("an error envelope carries no result: {envelope}"));
    }
    if envelope["id"] != json!(id) {
        return Err(format!("the envelope id is not `{id}`: {envelope}"));
    }
    let raw = serde_json::to_string(envelope).expect("an envelope serializes");
    if raw.contains("resultType") {
        return Err(format!(
            "an error envelope claims no result type: {envelope}"
        ));
    }
    Ok(())
}

/// A complete result: the revision's one completeness marker.
pub(super) fn check_complete_result(result: &Value) -> Result<(), String> {
    if result["resultType"] != json!("complete") {
        return Err(format!("the result is not complete: {result}"));
    }
    Ok(())
}

/// Discovery identity lives in `result._meta` and never at the top of the
/// result.
pub(super) fn check_discovery_identity(result: &Value) -> Result<(), String> {
    if result.get("serverInfo").is_some() {
        return Err(format!(
            "identity is never a top-level result field: {result}"
        ));
    }
    let info = &result["_meta"]["io.modelcontextprotocol/serverInfo"];
    if info["name"] != json!(env!("CARGO_PKG_NAME")) {
        return Err(format!("the identity name is the crate name: {result}"));
    }
    if info["version"] != json!(env!("CARGO_PKG_VERSION")) {
        return Err(format!(
            "the identity version is the crate version: {result}"
        ));
    }
    Ok(())
}

/// The payload one `tools/call` result carries: `structuredContent` when the
/// tool declares an output schema, otherwise the serialized payload in its one
/// text block. Both candidate transports read a payload through this, so a
/// domain claim is checked on the payload and not on either envelope's shape.
pub(super) fn read_payload(result: &Value) -> Result<Value, String> {
    if let Some(payload) = result.get("structuredContent") {
        return Ok(payload.clone());
    }
    let Some(text) = result["content"][0]["text"].as_str() else {
        return Err(format!("a tool result carries a payload: {result}"));
    };
    serde_json::from_str(text)
        .map_err(|error| format!("the text block is not the payload ({error}): {result}"))
}

/// A domain read that is not a tool error and carries `isError: false`.
fn check_read_ok(result: &Value) -> Result<(), String> {
    if result.get("isError") != Some(&json!(false)) {
        return Err(format!("the domain read is not an error: {result}"));
    }
    Ok(())
}

/// The `get_gist` domain read: the seeded item's gist, read as the
/// non-authoritative half.
pub(super) fn check_gist_read(result: &Value) -> Result<(), String> {
    check_read_ok(result)?;
    let payload = read_payload(result)?;
    if payload["item_id"] != json!(DOMAIN_ITEM_ID) {
        return Err(format!(
            "the read names the seeded item `{DOMAIN_ITEM_ID}`: {payload}"
        ));
    }
    if payload["authoritative"] != json!(false) {
        return Err(format!("a gist is never authoritative: {payload}"));
    }
    if payload["gist"] != json!(DOMAIN_GIST) {
        return Err(format!(
            "the read carries the stored gist `{DOMAIN_GIST}`: {payload}"
        ));
    }
    Ok(())
}

/// The `inspect` domain listing: exactly the one seeded item, whose row carries
/// that item's seeded category and epistemic kind, with a total that counts the
/// rows it lists, and the echo of the filter the store applied.
///
/// The row's content is checked, not only its identity, so a listing that lists
/// the right id beside a blank or foreign row cannot pass.
pub(super) fn check_domain_listing(result: &Value) -> Result<(), String> {
    check_read_ok(result)?;
    let payload = read_payload(result)?;
    let items = payload["items"]
        .as_array()
        .ok_or_else(|| format!("a listing carries an item array: {payload}"))?;
    let total = payload["total"].as_u64();
    if items.len() != 1 || total != Some(items.len() as u64) {
        return Err(format!(
            "the listing holds exactly the one seeded item, and its total counts its rows: {payload}"
        ));
    }
    if items[0]["item_id"] != json!(DOMAIN_ITEM_ID) {
        return Err(format!(
            "the listed row is the seeded item `{DOMAIN_ITEM_ID}`: {payload}"
        ));
    }
    if items[0]["category"] != json!(DOMAIN_CATEGORY)
        || items[0]["epistemic_kind"] != json!(DOMAIN_EPISTEMIC_KIND)
    {
        return Err(format!(
            "the listed row is a `{DOMAIN_CATEGORY}`/{DOMAIN_EPISTEMIC_KIND} row: {payload}"
        ));
    }
    if payload["include_discarded"] != json!(false) {
        return Err(format!(
            "a default listing excludes discarded rows: {payload}"
        ));
    }
    Ok(())
}

/// The advertised catalogue: every tool name the visible view lists, in the
/// order the view lists it.
///
/// The names come from the production view, so a walked listing is compared
/// with what the dispatcher advertises rather than with its own pages.
pub(super) fn catalogue_names(state: &McpState) -> Vec<String> {
    let view = crate::protocol::McpHandler::tools_list_visible(state)
        .expect("the fixture configuration advertises a tool view");
    view["tools"]
        .as_array()
        .expect("a tool view lists tools")
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .expect("an advertised tool has a name")
                .to_owned()
        })
        .collect()
}

/// A whole listing walk: more than one page, every catalogue tool exactly once
/// in catalogue order, a distinct non-empty cursor on every page but the last,
/// and no cursor on the last.
pub(super) fn check_listing_walk(pages: &[Value], catalogue: &[String]) -> Result<(), String> {
    if pages.len() < 2 {
        return Err(format!(
            "a walk covers more than one page, not {}",
            pages.len()
        ));
    }
    let last = pages.len() - 1;
    let mut walked: Vec<String> = Vec::new();
    let mut cursors: Vec<String> = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let tools = page["tools"]
            .as_array()
            .ok_or_else(|| format!("page {index} lists tools: {page}"))?;
        if tools.is_empty() {
            return Err(format!("page {index} of a walk lists no tools: {page}"));
        }
        for tool in tools {
            let name = tool["name"]
                .as_str()
                .ok_or_else(|| format!("page {index} names its tools: {tool}"))?;
            walked.push(name.to_owned());
        }
        let cursor = page.get("nextCursor");
        if index == last {
            if let Some(cursor) = cursor {
                return Err(format!(
                    "the terminal page carries the cursor `{cursor}`: {page}"
                ));
            }
            continue;
        }
        let Some(cursor) = cursor.and_then(Value::as_str).filter(|c| !c.is_empty()) else {
            return Err(format!(
                "page {index} carries no usable `nextCursor`: {page}"
            ));
        };
        if cursors.iter().any(|seen| seen == cursor) {
            return Err(format!("page {index} repeats the cursor `{cursor}`"));
        }
        cursors.push(cursor.to_owned());
    }
    for (index, name) in walked.iter().enumerate() {
        if walked[..index].contains(name) {
            return Err(format!("`{name}` appears twice in the walk"));
        }
    }
    if walked.len() != catalogue.len() {
        return Err(format!(
            "the walk covers {} of the {} catalogue tools: {walked:?}",
            walked.len(),
            catalogue.len()
        ));
    }
    if walked.as_slice() != catalogue {
        return Err(format!(
            "the walk is not the catalogue order: {walked:?} against {catalogue:?}"
        ));
    }
    Ok(())
}
