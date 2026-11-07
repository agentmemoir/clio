// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `tool_search` discovery tool: turning one
//! argument object into a ranked page of the business tool catalog.
//!
//! ## Owns
//! - The `tool_search` name, its argument validation, and its result page
//! - The tokenizer and the raw overlap score that ranks a tool
//! - The `tools/call` success envelope it publishes
//!
//! ## Does not own
//! - Catalog membership or catalog order (see `schema`)
//! - Tool group membership (see `clio_types::group_of`)
//! - The discovery gate, which `protocol` reads under the config lock
//!
//! ## Boundary
//!
//! Changes to how a query ranks or filters the catalog should normally require
//! changes here. Changes to the catalog or to the config gate should not.
//!
//! Keep this module focused on one search request and its page.

use crate::protocol::{RpcError, invalid_params, unknown_tool};
use crate::runtime::McpState;
use crate::schema::catalog_defs;
use crate::schema_defs::ToolDef;
use clio_types::{ToolGroup, group_of};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The published name of the discovery search tool.
pub(crate) const NAME: &str = "tool_search";

/// The page size a request gets when it asks for none.
const DEFAULT_LIMIT: usize = 10;

/// The largest page a request can get, however high it asks.
const MAX_LIMIT: usize = 25;

/// The argument keys `tool_search` accepts.
const KEYS: [&str; 3] = ["query", "limit", "family"];

/// Validate one argument object and return its result page.
///
/// # Errors
/// The arguments are not an object, carry an unknown key, omit `query`, hold a
/// blank or non-string `query`, name a non-integer or sub-one `limit`, or name a
/// family this build does not know.
pub(crate) fn call(args: &Value) -> Result<Value, RpcError> {
    let object = args
        .as_object()
        .ok_or_else(|| invalid_params("tool_search arguments must be an object"))?;
    for key in object.keys() {
        if !KEYS.contains(&key.as_str()) {
            return Err(invalid_params(format!(
                "tool_search does not accept the `{key}` argument"
            )));
        }
    }
    let query = match object.get("query") {
        None => return Err(invalid_params("tool_search.query is required")),
        Some(Value::String(query)) if !query.trim().is_empty() => query.as_str(),
        Some(Value::String(_)) => {
            return Err(invalid_params(
                "tool_search.query must hold a non-whitespace character",
            ));
        }
        Some(_) => return Err(invalid_params("tool_search.query must be a string")),
    };
    let limit = limit_of(object.get("limit"))?;
    let family = family_of(object.get("family"))?;
    let query_tokens = tokens(query);
    let mut scored: Vec<(usize, ToolDef, ToolGroup)> = Vec::new();
    for def in catalog_defs() {
        let Some(group) = group_of(def.name) else {
            continue;
        };
        if family.is_some_and(|wanted| wanted != group) {
            continue;
        }
        let mut tool_tokens = tokens(def.name);
        tool_tokens.extend(tokens(def.description));
        let score = score(&query_tokens, &tool_tokens);
        if score > 0 {
            scored.push((score, def, group));
        }
    }
    // A stable descending sort seeded from the catalog order keeps equal scores
    // in catalog order; alphabetical order is not the tie-break.
    scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let tools: Vec<Value> = scored
        .iter()
        .take(limit)
        .map(|(_, def, group)| {
            json!({
                "name": def.name,
                "description": def.description,
                "group": group.as_str(),
            })
        })
        .collect();
    Ok(json!({ "tools": tools }))
}

/// True when `mcp.discovery` turns the discovery tools on for this state.
///
/// An absent key reads as off, so a deployment that never sets it advertises
/// and dispatches exactly the tools it did before the key existed.
pub(crate) fn enabled(state: &McpState) -> bool {
    let runtime = state.config.lock().expect("config runtime mutex poisoned");
    runtime.mcp_discovery()
}

/// The `tools/call` success envelope for one result page.
///
/// The tool declares an output schema, so the page travels once in
/// `structuredContent` and `content` carries the short status line. Success is
/// this function's own fact, never a field of the page.
#[allow(clippy::needless_pass_by_value)] // the published helper owns the page it advertises
pub(crate) fn envelope(result: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": "ok" }],
        "isError": false,
        "structuredContent": result,
    })
}

/// Route one `tools/call` for this tool.
///
/// `tool_search` dispatches over the whole catalog, so it serves a name outside
/// the bound set while the discovery setting keeps it on.
///
/// # Errors
/// Discovery is off, or the arguments fail the `tool_search` contract.
pub(crate) fn route(state: &McpState, args: &Value) -> Result<Value, RpcError> {
    if !enabled(state) {
        return Err(unknown_tool(NAME));
    }
    Ok(envelope(call(args)?))
}

/// The requested page size, clamped to the maximum, or the default when absent.
fn limit_of(value: Option<&Value>) -> Result<usize, RpcError> {
    let Some(value) = value else {
        return Ok(DEFAULT_LIMIT);
    };
    let raw = value
        .as_u64()
        .filter(|limit| *limit >= 1)
        .ok_or_else(|| invalid_params("tool_search.limit must be an integer of at least 1"))?;
    Ok(usize::try_from(raw).unwrap_or(MAX_LIMIT).min(MAX_LIMIT))
}

/// The requested family, or `None` when the arguments name none.
fn family_of(value: Option<&Value>) -> Result<Option<ToolGroup>, RpcError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let name = value
        .as_str()
        .ok_or_else(|| invalid_params("tool_search.family must be a string"))?;
    let group = ToolGroup::from_str(name).ok_or_else(|| {
        invalid_params(format!(
            "tool_search.family `{name}` is not one of {}",
            ToolGroup::ALL.map(ToolGroup::as_str).join("|")
        ))
    })?;
    Ok(Some(group))
}

/// Split one text into the lowercase alphanumeric words the match uses.
pub(crate) fn tokens(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The raw number of query tokens the tool's tokens also hold.
pub(crate) fn score(query: &BTreeSet<String>, tool: &BTreeSet<String>) -> usize {
    query.intersection(tool).count()
}

#[cfg(test)]
#[path = "catalog_search_tests.rs"]
mod catalog_search_tests;

#[cfg(test)]
#[path = "catalog_search_ranking_tests.rs"]
mod catalog_search_ranking_tests;
