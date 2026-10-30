// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio stats` human text view: the bank
//! headline counts plus every breakdown group the JSON report carries
//! (tier, category, epistemic kind, age band, provenance class, pressure).
//!
//! ## Owns
//! - The `stats` text view over the MCP `stats` payload
//! - The `label count` pairing and the empty-group wording for one breakdown
//!
//! ## Does not own
//! - Stats computation and report shape (see `clio-compliance`)
//! - JSON emission or output-mode selection (see `cli_output`)
//! - The other read verbs' text views (see `cli_read_render`)
//!
//! ## Boundary
//!
//! Changes to the `stats` human text view should normally require changes here.
//! Changes to the report shape should not require changes here.
//!
//! Keep this module focused on the `stats` text view.

use serde_json::Value;

/// The breakdown groups the JSON report carries, in report order.
const GROUPS: [&str; 6] = [
    "by_tier",
    "by_category",
    "by_epistemic_kind",
    "by_age_band",
    "by_provenance_class",
    "pressure_counts",
];

/// Render the `stats` payload as deterministic human text.
///
/// Every field the JSON report carries gets a line, so the text form carries
/// the same fields as the JSON form.
pub(crate) fn render_stats(payload: &Value) -> String {
    let number = |key: &str| match payload.get(key).and_then(Value::as_u64) {
        Some(n) => n.to_string(),
        None => "?".to_owned(),
    };
    let mut lines = vec![
        format!("bank: {}", text(payload, "bank")),
        format!("items_total: {}", number("items_total")),
        format!("items_active: {}", number("items_active")),
        format!("items_discarded: {}", number("items_discarded")),
    ];
    for group in GROUPS {
        lines.push(format!("{group}: {}", counts_line(payload, group)));
    }
    lines.join("\n")
}

/// One string field, `?` when absent: an absent field is not an empty bank.
fn text<'a>(payload: &'a Value, key: &'a str) -> &'a str {
    payload.get(key).and_then(Value::as_str).unwrap_or("?")
}

/// One breakdown group as `label count` pairs.
///
/// `?` when the field is absent, `none` when the report carries an empty
/// group, so an empty group is named rather than silently dropped.
fn counts_line(payload: &Value, key: &str) -> String {
    let Some(buckets) = payload.get(key).and_then(Value::as_array) else {
        return "?".to_owned();
    };
    if buckets.is_empty() {
        return "none".to_owned();
    }
    buckets
        .iter()
        .filter_map(|bucket| {
            let label = bucket.get("label").and_then(Value::as_str)?;
            let count = bucket.get("count").and_then(Value::as_u64)?;
            Some(format!("{label} {count}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "cli_read_render_stats_tests.rs"]
mod cli_read_render_stats_tests;
