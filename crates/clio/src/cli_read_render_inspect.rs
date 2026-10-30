// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio inspect` human text view: the
//! page header, the response metadata the JSON payload carries (total, limit,
//! offset, `include_discarded`, truncated), the optional supersession section,
//! and one block per item row with every field the row carries, including the
//! optional `context` and `freshness` blocks.
//!
//! ## Owns
//! - The `inspect` text view over the MCP `inspect` payload
//! - The per-row field lines and the compact freshness/supersession summaries
//!
//! ## Does not own
//! - The inspect tool semantics and row shape (see `clio-mcp` / `clio-store`)
//! - JSON emission or output-mode selection (see `cli_output`)
//! - The other read verbs' text views (see `cli_read_render`)
//!
//! ## Boundary
//!
//! Changes to the `inspect` human text view should normally require changes
//! here. Changes to the row shape should not require changes here.
//!
//! Keep this module focused on the `inspect` text view.

use serde_json::Value;

/// The response metadata fields the JSON payload carries, in payload order.
const META_FIELDS: [&str; 5] = ["total", "limit", "offset", "include_discarded", "truncated"];

/// The row fields the tool serializes for one item, in row order; `context`
/// and `freshness` are appended by the tool and rendered after these.
const ROW_FIELDS: [&str; 17] = [
    "item_id",
    "kind",
    "category",
    "episodic_type",
    "epistemic_kind",
    "source_type",
    "confidence",
    "admission_score",
    "source_ref",
    "valid_from",
    "valid_until",
    "tx_from",
    "tx_until",
    "discarded",
    "archived",
    "created_at",
    "updated_at",
];

/// Render the `inspect` payload as deterministic human text.
///
/// Every field the JSON payload carries gets a line, so the text form carries
/// the same fields as the JSON form.
pub(crate) fn render_inspect(payload: &Value) -> String {
    let items = payload.get("items").and_then(Value::as_array);
    let returned = items.map_or(0, Vec::len);
    let total = payload
        .get("total")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(returned);
    let mut header = format!("items: {returned} of {total}");
    // The page the caller asked for was lowered to the cap; name both values
    // so the header never silently disagrees with the request.
    if let (Some(limit), Some(requested)) = (
        payload.get("limit").and_then(Value::as_u64),
        payload.get("requested_limit").and_then(Value::as_u64),
    ) {
        use std::fmt::Write as _;
        let _ = write!(header, " (limit {limit}, lowered from {requested})");
    }
    let mut lines = vec![header];
    lines.extend(meta_lines(payload));
    if let Some(items) = items {
        for item in items {
            lines.extend(row_lines(item));
        }
    }
    if let Some(supersession) = payload.get("supersession") {
        lines.push(format!("supersession: {}", compact(supersession)));
    }
    lines.join("\n")
}

/// One line per metadata field the payload carries.
fn meta_lines(payload: &Value) -> Vec<String> {
    META_FIELDS
        .iter()
        .filter_map(|key| {
            payload
                .get(*key)
                .map(|value| format!("{key}: {}", leaf(value)))
        })
        .collect()
}

/// The lines for one item row: the id, then every field the row carries.
fn row_lines(item: &Value) -> Vec<String> {
    let id = item.get("item_id").and_then(Value::as_str).unwrap_or("?");
    let mut lines = vec![id.to_owned()];
    for key in ROW_FIELDS.iter().skip(1) {
        if let Some(value) = item.get(key) {
            lines.push(format!("  {key}: {}", leaf(value)));
        }
    }
    if let Some(context) = item.get("context") {
        lines.push(format!("  context: {}", leaf(context)));
    }
    if let Some(freshness) = item.get("freshness") {
        lines.push(format!("  freshness: {}", compact(freshness)));
    }
    // A field outside the known row shape still gets a line, so a field the
    // payload adds later cannot silently disappear from the text view.
    if let Some(fields) = item.as_object() {
        for (key, value) in fields {
            if !is_known_row_field(key) {
                lines.push(format!("  {key}: {}", compact(value)));
            }
        }
    }
    lines
}

/// True when the row field is rendered by its own block above.
fn is_known_row_field(key: &str) -> bool {
    ROW_FIELDS.contains(&key) || matches!(key, "context" | "freshness")
}

/// One compact line over an object or array value: `k v` pairs for an object,
/// joined entries for an array, the scalar text otherwise.
fn compact(value: &Value) -> String {
    match value {
        Value::Object(fields) => fields
            .iter()
            .map(|(key, field)| format!("{key} {}", leaf(field)))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Array(entries) => entries.iter().map(leaf).collect::<Vec<_>>().join("; "),
        other => leaf(other),
    }
}

/// The text for one field value: strings bare, `null` as `null`, numbers and
/// booleans as their text, a nested value as its compact JSON.
fn leaf(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_owned(),
        Value::Object(_) | Value::Array(_) => value.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
#[path = "cli_read_render_inspect_tests.rs"]
mod cli_read_render_inspect_tests;
