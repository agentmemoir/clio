// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` graph and workspace-inspection
//! read commands: `graph query`, `associations`, `persona get`, and
//! `maintenance status`.
//!
//! ## Owns
//! - Flag decoding for the graph traversal, association listing, persona
//!   document, and maintenance-status verbs, plus their human text views
//!
//! ## Does not own
//! - Graph/persona/maintenance semantics (see `clio-store` / `clio-persona` /
//!   `clio-write`)
//! - The read engine and in-process call bridge (see `cli_read`)
//!
//! ## Boundary
//!
//! Changes to a graph/persona/maintenance read command's flags or views should
//! normally require changes here. Changes to their storage should not.
//!
//! Keep this module focused on the graph and workspace-inspection read commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    no_positionals, one_positional, parse_f64, parse_u64, read_args, validate_choice,
};
use crate::cli_read_group::ReadGroup;
use serde_json::Value;

/// The graph and workspace-inspection read commands.
pub(crate) struct GraphGroup;

impl ReadGroup for GraphGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &[
            "graph query",
            "associations",
            "persona get",
            "maintenance status",
        ]
    }

    // The registry record owns each verb's accepted set, so the parser and the
    // usage line cannot disagree; a name with no record accepts only the globals.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(record) => record.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
        match verb {
            "graph query" => graph_query_call(parsed),
            "associations" => associations_call(parsed),
            "persona get" => persona_get_call(parsed),
            _ => maintenance_status_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "graph query" => render_id_list("nodes", payload),
            "associations" => render_associations(payload),
            "persona get" => render_persona(payload),
            _ => render_maintenance(payload),
        }
    }
}

fn graph_query_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let seed_id = one_positional(parsed, "graph query", "SEED_ID")?;
    let mut args = read_args(parsed);
    args.insert("seed_id".into(), Value::from(seed_id));
    if let Some(hops) = parsed.value("max-hops") {
        args.insert("max_hops".into(), Value::from(parse_u64("max-hops", hops)?));
    }
    if let Some(edge_type) = parsed.value("edge-type") {
        args.insert(
            "edge_type".into(),
            Value::from(validate_choice(
                "edge-type",
                edge_type,
                &["coactivation", "explicit"],
            )?),
        );
    }
    if let Some(weight) = parsed.value("min-weight") {
        args.insert(
            "min_weight".into(),
            Value::from(parse_f64("min-weight", weight)?),
        );
    }
    Ok(("graph_query", Value::Object(args)))
}

fn associations_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let item_id = one_positional(parsed, "associations", "ITEM_ID")?;
    let mut args = read_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    if let Some(weight) = parsed.value("min-weight") {
        args.insert(
            "min_weight".into(),
            Value::from(parse_f64("min-weight", weight)?),
        );
    }
    Ok(("associations", Value::Object(args)))
}

fn persona_get_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    let mut args = read_args(parsed);
    if let Some(budget) = parsed.value("budget-tokens") {
        args.insert(
            "budget_tokens".into(),
            Value::from(parse_u64("budget-tokens", budget)?),
        );
    }
    Ok(("persona_get", Value::Object(args)))
}

fn maintenance_status_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    let mut args = read_args(parsed);
    if let Some(item_id) = parsed.value("item-id") {
        args.insert("item_id".into(), Value::from(item_id));
    }
    Ok(("maintenance_status", Value::Object(args)))
}

fn render_id_list(key: &str, payload: &Value) -> String {
    let items = payload.get(key).and_then(Value::as_array);
    let returned = items.map_or(0, Vec::len);
    let mut lines = vec![format!("{key}: {returned}")];
    if let Some(items) = items {
        for item in items {
            let id = item
                .get("id")
                .or_else(|| item.get("item_id"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            lines.push(id.to_owned());
        }
    }
    lines.join("\n")
}

fn render_associations(payload: &Value) -> String {
    let rows = payload.get("associations").and_then(Value::as_array);
    let returned = rows.map_or(0, Vec::len);
    let mut lines = vec![format!("associations: {returned}")];
    if let Some(rows) = rows {
        for row in rows {
            let neighbor = row
                .get("neighbor_id")
                .and_then(Value::as_str)
                .unwrap_or("?");
            lines.push(neighbor.to_owned());
        }
    }
    lines.join("\n")
}

fn render_persona(payload: &Value) -> String {
    let document = payload.get("document");
    let count = |key: &str| {
        document
            .and_then(|document| document.get(key))
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    let dropped = payload
        .get("dropped_keys")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    format!(
        "stable: {}\npreferences: {}\ndropped_keys: {dropped}",
        count("stable"),
        count("preferences")
    )
}

fn render_maintenance(payload: &Value) -> String {
    let mut lines = vec![format!(
        "leaf_queryable: {}",
        payload
            .get("leaf_queryable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    )];
    if let Some(item_id) = payload.get("item_id").and_then(Value::as_str) {
        lines.push(format!("item_id: {item_id}"));
    }
    for key in ["dirty_ancestors", "refreshing"] {
        let count = payload
            .get(key)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        lines.push(format!("{key}: {count}"));
    }
    for key in ["time_to_queryable_ms", "structural_maintenance_ms"] {
        let value = payload.get(key).and_then(Value::as_u64).unwrap_or(0);
        lines.push(format!("{key}: {value}"));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "cli_read_graph_tests.rs"]
mod tests;
