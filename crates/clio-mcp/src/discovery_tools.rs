// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for rendering one catalog tool's published
//! definition for a `tool_describe` call.
//!
//! ## Owns
//! - The `tool_describe` name and its `arguments` validation
//! - The exact refusal for a name the catalog does not define
//! - The definition this call returns, read from the full catalog
//! - The discovery gate `protocol` reads under the config lock
//!
//! ## Does not own
//! - The catalog itself or how a definition renders (see `schema` and
//!   `schema_defs`)
//! - Tool dispatch or binding (see `protocol`)
//! - The embedded declaration of `tool_describe` (see `discovery_tool_defs`)
//!
//! ## Boundary
//!
//! Changes to what `tool_describe` accepts or returns should normally require
//! changes here. Changes to a tool's parameters should not require changes
//! here.
//!
//! Keep this module focused on describing one catalog tool.

use crate::protocol::{RpcError, invalid_params, unknown_tool};
use crate::runtime::McpState;
use crate::schema::catalog_def;
use serde_json::{Value, json};

/// The discovery tool that returns one catalog definition.
pub(crate) const TOOL_DESCRIBE: &str = "tool_describe";

/// Render the catalog definition named by `args`.
///
/// The lookup reads the whole catalog, so the selected tool profile cannot
/// narrow or hide the answer.
///
/// # Errors
/// `args` does not carry exactly one non-empty string `name`, or the name is
/// absent from the catalog.
pub(crate) fn describe(args: &Value) -> Result<Value, RpcError> {
    let name = exact_name(args)?;
    let def = catalog_def(name).ok_or_else(|| unknown_tool(name))?;
    Ok(def.to_tool())
}

/// True when `mcp.discovery` turns the discovery tools on for this state.
///
/// An absent key reads as off, so a deployment that never sets it advertises
/// and dispatches exactly the tools it did before the key existed.
pub(crate) fn enabled(state: &McpState) -> bool {
    let runtime = state.config.lock().expect("config runtime mutex poisoned");
    runtime.mcp_discovery()
}

/// Route a `tools/call` name this module owns, or `None` for any other name.
///
/// A bound tool that later takes this name keeps the normal dispatch path, so
/// the guard names binding explicitly instead of assuming the discovery tool is
/// never bound. Discovery off refuses the call with the same unknown-tool error
/// the unbound set answers.
pub(crate) fn route(state: &McpState, name: &str, args: &Value) -> Option<Result<Value, RpcError>> {
    if name != TOOL_DESCRIBE || crate::bound_tools().contains(&name) {
        return None;
    }
    if !enabled(state) {
        return Some(Err(unknown_tool(name)));
    }
    Some(describe(args).map(|payload| {
        json!({
            "content": [{ "type": "text", "text": "ok" }],
            "isError": false,
            "structuredContent": payload
        })
    }))
}

/// The single `name` field the published schema declares.
fn exact_name(args: &Value) -> Result<&str, RpcError> {
    let Some(fields) = args.as_object() else {
        return Err(invalid_params("tool_describe.arguments must be an object"));
    };
    if let Some(extra) = fields.keys().find(|key| key.as_str() != "name") {
        return Err(invalid_params(format!(
            "unknown argument `{extra}` for `tool_describe`"
        )));
    }
    match fields.get("name") {
        None => Err(invalid_params("name is required")),
        Some(Value::String(name)) if !name.is_empty() => Ok(name.as_str()),
        Some(Value::String(_)) => Err(invalid_params("name must not be empty")),
        Some(_) => Err(invalid_params("name must be a string")),
    }
}

#[cfg(test)]
#[path = "discovery_tools_tests.rs"]
mod discovery_tools_tests;
