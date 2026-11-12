// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for paginating one visible `tools/list` view:
//! the page-size extension, the opaque cursor, and the slice that carries
//! `nextCursor` only while more tools remain.
//!
//! ## Owns
//! - The default page size, the maximum page size, and the request key that
//!   selects a page size
//! - Building the advertised view from the live configuration on first use:
//!   the bound catalog narrowed to the effective group selection, plus the
//!   discovery entries the gate turns on
//! - Cursor encoding and decoding, including its format version
//! - The page slice and its optional `nextCursor`
//!
//! ## Does not own
//! - The immutable snapshot value and its digest (see `tool_snapshot`)
//! - Installing or replacing the snapshot (see `tool_signals`)
//! - Tool semantics and dispatch (see `write_tools` and `protocol`)
//! - The catalog hash convention (see `schema`)
//!
//! ## Boundary
//!
//! Changes to how a listing is sliced, or to the cursor it carries, should
//! normally require changes here. Changes to profile membership, the catalog,
//! or transport framing should not require changes here.
//!
//! Keep this module focused on turning one visible tool list into pages.

use crate::protocol::{ERR_INTERNAL, RpcError, invalid_params};
use crate::runtime::McpState;
use crate::tool_snapshot::ToolSnapshot;
use base64::Engine;
use clio_types::{ToolGroup, ToolPreset, tools_of};
use serde_json::{Value, json};
use std::sync::Arc;

/// The page size a request gets when it asks for none.
pub(crate) const DEFAULT_PAGE_SIZE: usize = 25;

/// The largest page a request can get, however high it asks.
pub(crate) const MAX_PAGE_SIZE: usize = 100;

/// The request metadata key that selects a page size. This key is a Clio
/// extension, not part of the protocol's request shape.
pub(crate) const PAGE_SIZE_KEY: &str = "com.agentmemoir/pageSize";

/// The cursor format version this build writes and accepts.
const CURSOR_VERSION: u64 = 1;

/// The identity of one visible view, read under one configuration lock.
///
/// One snapshot holds every digest input, so a configuration change between two
/// reads cannot bind a cursor to a state that never existed.
#[derive(Debug, Clone)]
pub(crate) struct ToolView {
    /// The preset that names the view, or `None` when an installed selection
    /// governs it and no document preset applies.
    pub(crate) preset: Option<ToolPreset>,
    /// The effective group selection, with its dependency closure applied.
    pub(crate) groups: Vec<ToolGroup>,
    /// The effective `mcp.revision`, or the pinned protocol revision when the
    /// configuration sets none.
    ///
    /// A client cannot ask for a different revision, and a protocol-version
    /// change does not alter the tools. This input can only invalidate cursors
    /// issued before the revision changed; it never detects a changed view.
    pub(crate) revision: String,
}

/// The decoded position one cursor carries.
struct Cursor {
    digest: String,
    size: usize,
    offset: usize,
}

/// The catalogue a dispatch is bounded by.
///
/// A client sees the view `tools/list` advertises. The command line names its
/// own verbs and serves the whole bound catalogue, so its dispatch is bounded
/// by that catalogue and not by a listing it never asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ToolScope {
    /// The view `tools/list` advertises, which bounds a client's `tools/call`.
    #[default]
    Advertised,
    /// The whole bound catalogue, which the command line dispatches against.
    Bound,
}

/// The installed snapshot, seeding it from the live configuration on first use.
///
/// Every caller shares one immutable value, so a listing and a listener agree
/// on the view even when the configuration changes between their calls.
pub(crate) fn visible_view(state: &McpState) -> Result<Arc<ToolSnapshot>, RpcError> {
    if let Some(snapshot) = state.tool_signals.current() {
        return Ok(snapshot);
    }
    Ok(state.tool_signals.seed(live_snapshot(state)?))
}

/// Build a snapshot from the live configuration, the effective selection, and
/// the visible definitions, all read under one configuration lock.
///
/// The discovery entries sit first, so a default first page carries all three
/// whatever preset governs the view; the catalog follows them in catalog order.
///
/// The groups come from the effective selection, so an installed `--tools`
/// selection wins over `mcp.tool_profile`. When a selection governs the view,
/// `mcp_tool_groups` answers without the document, so a missing or unknown
/// document preset cannot fail the request; the profile is then absent from the
/// snapshot.
fn live_snapshot(state: &McpState) -> Result<ToolSnapshot, RpcError> {
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
    let bound = crate::bound_tools();
    let mut tools: Vec<Value> = if discovery {
        crate::discovery_tool_defs::entries()
    } else {
        Vec::new()
    };
    tools.extend(
        crate::schema::catalog_defs()
            .iter()
            .filter(|def| bound.contains(&def.name) && visible.contains(&def.name))
            .map(crate::schema_defs::ToolDef::to_tool),
    );
    let view = ToolView {
        preset: preset.ok(),
        groups,
        revision: revision.unwrap_or_else(|| crate::MCP_PROTOCOL_REVISION.to_owned()),
    };
    Ok(ToolSnapshot::build(view, tools))
}

/// Slice one visible tool list into the page a request selects.
///
/// The first request of a walk carries no cursor, so it starts at offset zero
/// with the requested size. Later requests carry a cursor, which holds the
/// view digest, the page size, and the next offset. A cursor is refused unless
/// its offset is inside the view, so an offset at or past the end never answers
/// an empty page. A page that ends before the list does carries `nextCursor`;
/// the last page omits it. An empty view answers an initial request with an
/// empty page and no cursor.
pub(crate) fn page(snapshot: &ToolSnapshot, params: Option<&Value>) -> Result<Value, RpcError> {
    let digest = snapshot.digest();
    let tools = snapshot.tools();
    let requested = requested_size(params)?;
    let (size, offset) = match requested_cursor(params)? {
        Some(encoded) => {
            let cursor = decode_cursor(encoded)?;
            if cursor.digest != digest {
                return Err(invalid_params(
                    "tools/list cursor was issued for a different tool view",
                ));
            }
            if requested.is_some_and(|size| size != cursor.size) {
                return Err(invalid_params(format!(
                    "`{PAGE_SIZE_KEY}` disagrees with the page size in the cursor"
                )));
            }
            if cursor.offset >= tools.len() {
                return Err(invalid_params(
                    "tools/list cursor points at or past the end of the tool view",
                ));
            }
            (cursor.size, cursor.offset)
        }
        None => (requested.unwrap_or(DEFAULT_PAGE_SIZE), 0),
    };
    let end = offset.saturating_add(size).min(tools.len());
    let mut result = json!({ "tools": tools[offset..end].to_vec() });
    if end < tools.len() {
        result["nextCursor"] = Value::String(encode_cursor(digest, size, end));
    }
    Ok(result)
}

/// The requested page size, clamped to the maximum, or `None` when absent.
fn requested_size(params: Option<&Value>) -> Result<Option<usize>, RpcError> {
    let Some(value) = params
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(PAGE_SIZE_KEY))
    else {
        return Ok(None);
    };
    let raw = value.as_u64().filter(|size| *size >= 1).ok_or_else(|| {
        invalid_params(format!(
            "`{PAGE_SIZE_KEY}` must be an integer of at least 1"
        ))
    })?;
    Ok(Some(
        usize::try_from(raw)
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE),
    ))
}

/// The cursor a request supplies, if any.
fn requested_cursor(params: Option<&Value>) -> Result<Option<&str>, RpcError> {
    match params.and_then(|params| params.get("cursor")) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(cursor)) => Ok(Some(cursor.as_str())),
        Some(_) => Err(invalid_params("tools/list cursor must be a string")),
    }
}

/// Encode one cursor as base64url without padding over a JSON tuple:
/// format version, view digest, page size, next offset.
fn encode_cursor(digest: &str, size: usize, offset: usize) -> String {
    let payload = json!([CURSOR_VERSION, digest, size, offset]);
    let bytes = serde_json::to_vec(&payload).expect("a cursor tuple serializes");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode one cursor, refusing a malformed value, a wrong format version, or a
/// page size outside the accepted range.
fn decode_cursor(encoded: &str) -> Result<Cursor, RpcError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid_params("tools/list cursor is not valid base64url"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| invalid_params("tools/list cursor is not a JSON tuple"))?;
    let fields = value
        .as_array()
        .filter(|fields| fields.len() == 4)
        .ok_or_else(|| invalid_params("tools/list cursor is not a four-field tuple"))?;
    if fields[0].as_u64() != Some(CURSOR_VERSION) {
        return Err(invalid_params(
            "tools/list cursor has an unsupported format version",
        ));
    }
    let digest = fields[1]
        .as_str()
        .ok_or_else(|| invalid_params("tools/list cursor digest is not a string"))?
        .to_owned();
    let size = cursor_integer(&fields[2])?;
    if !(1..=MAX_PAGE_SIZE).contains(&size) {
        return Err(invalid_params(
            "tools/list cursor carries a page size out of range",
        ));
    }
    let offset = cursor_integer(&fields[3])?;
    Ok(Cursor {
        digest,
        size,
        offset,
    })
}

/// One non-negative integer field of a cursor tuple.
fn cursor_integer(value: &Value) -> Result<usize, RpcError> {
    let raw = value
        .as_u64()
        .ok_or_else(|| invalid_params("tools/list cursor carries a non-integer field"))?;
    usize::try_from(raw)
        .map_err(|_| invalid_params("tools/list cursor carries an out-of-range field"))
}

impl McpState {
    /// Install the tool view the live configuration describes now.
    ///
    /// Returns true only when the installed digest changed, so a caller can
    /// tell a real change from a repeat. A view that cannot be built installs
    /// nothing and returns false.
    pub(crate) fn replace_tool_view(&self) -> bool {
        match live_snapshot(self) {
            Ok(snapshot) => self.tool_signals.replace(snapshot).is_some(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
#[path = "tools_page_tests.rs"]
mod tools_page_tests;
