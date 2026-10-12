// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` history and task read commands:
//! `task get`, `task history`, `failure list`, `memtree query`, `memtree get`,
//! and `history temporal`.
//!
//! ## Owns
//! - Flag decoding for the task/failure/MemTree/temporal verbs into MCP tool
//!   arguments, and their in-band `truncated` markers and human text views
//! - Secret masking and inline prose scrubbing over history view payloads
//!
//! ## Does not own
//! - History/MemTree semantics (see `clio-history` / `clio-write`)
//! - The read engine and in-process call bridge (see `cli_read`)
//!
//! ## Boundary
//!
//! Changes to a history/task read command's flags or views should normally
//! require changes here. Changes to history storage should not require changes here.
//!
//! Keep this module focused on the history and task read commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    mark_truncated, no_positionals, one_positional, parse_u64, read_args, validate_time_axis,
};
use crate::cli_read_group::ReadGroup;
use clio_config::secret::{mask_string_secrets, scrub_inline_secrets};
use serde_json::{Map, Value};

/// The history and task read commands.
pub(crate) struct HistoryGroup;

impl ReadGroup for HistoryGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &[
            "task get",
            "task history",
            "failure list",
            "memtree query",
            "memtree get",
            "history temporal",
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
            "task get" => task_get_call(parsed),
            "task history" => task_history_call(parsed),
            "failure list" => failure_list_call(parsed),
            "memtree query" => memtree_query_call(parsed),
            "memtree get" => memtree_get_call(parsed),
            _ => temporal_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "task get" => render_task_get(payload),
            "task history" => render_task_history(payload),
            "failure list" => render_failure_list(payload),
            "memtree query" => render_node_list("nodes", payload),
            "memtree get" => render_node(payload),
            _ => render_temporal(payload),
        }
    }

    fn decorate(&self, verb: &str, parsed: &ParsedArgs, _arguments: &Value, payload: &mut Value) {
        // Defence in depth: history views echo task definitions, failure
        // lessons, and gists, so mask secret-keyed string fields and scrub
        // inline `key: secret` prose before anything leaves the command.
        //
        // Only string leaves are masked: `failure list` carries the non-secret
        // numeric `lesson_tokens` count, whose key merely contains the `token`
        // marker. Rewriting it would change its type and break the contract
        // that a view returns the same structured result as its tool.
        mask_string_secrets("", payload);
        scrub_prose_secrets(payload);
        match verb {
            "task history" => mark_truncated(payload, "records"),
            "failure list" => mark_truncated(payload, "failures"),
            "memtree query" => {
                let returned = payload
                    .get("nodes")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let truncated = parsed
                    .value("limit")
                    .and_then(|raw| raw.parse::<usize>().ok())
                    .is_some_and(|limit| returned >= limit);
                payload["truncated"] = Value::Bool(truncated);
            }
            _ => {}
        }
    }
}

/// Scrub inline `key: secret` prose from every string leaf of a payload.
fn scrub_prose_secrets(value: &mut Value) {
    match value {
        Value::String(text) => {
            let scrubbed = scrub_inline_secrets(text);
            *text = scrubbed;
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                scrub_prose_secrets(child);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                scrub_prose_secrets(item);
            }
        }
        _ => {}
    }
}

fn task_get_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let task_id = one_positional(parsed, "task get", "TASK_ID")?;
    let mut args = read_args(parsed);
    args.insert("task_id".into(), Value::from(task_id));
    Ok(("task_get", Value::Object(args)))
}

fn task_history_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let task_id = one_positional(parsed, "task history", "TASK_ID")?;
    let mut args = read_args(parsed);
    args.insert("task_id".into(), Value::from(task_id));
    if let Some(limit) = parsed.value("limit") {
        args.insert("limit".into(), Value::from(parse_u64("limit", limit)?));
    }
    Ok(("task_history", Value::Object(args)))
}

fn failure_list_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let query = one_positional(parsed, "failure list", "TASK_ID_OR_QUERY")?;
    let mut args = read_args(parsed);
    args.insert("task_id_or_query".into(), Value::from(query));
    if let Some(limit) = parsed.value("limit") {
        args.insert("limit".into(), Value::from(parse_u64("limit", limit)?));
    }
    Ok(("failures_for_task", Value::Object(args)))
}

fn memtree_query_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    let mut args = read_args(parsed);
    let (start, end) = (parsed.value("time-start"), parsed.value("time-end"));
    if start.is_some() || end.is_some() {
        let mut range = Map::new();
        if let Some(start) = start {
            range.insert("start".into(), Value::from(start));
        }
        if let Some(end) = end {
            range.insert("end".into(), Value::from(end));
        }
        args.insert("time_range".into(), Value::Object(range));
    }
    if let Some(task_id) = parsed.value("task-id") {
        args.insert("task_id".into(), Value::from(task_id));
    }
    if let Some(depth) = parsed.value("depth") {
        args.insert("depth".into(), Value::from(parse_u64("depth", depth)?));
    }
    if let Some(limit) = parsed.value("limit") {
        args.insert("limit".into(), Value::from(parse_u64("limit", limit)?));
    }
    Ok(("memtree_query", Value::Object(args)))
}

fn memtree_get_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let node_id = one_positional(parsed, "memtree get", "NODE_ID")?;
    let mut args = read_args(parsed);
    args.insert("node_id".into(), Value::from(node_id));
    Ok(("memtree_get", Value::Object(args)))
}

fn temporal_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let target = one_positional(parsed, "history temporal", "TARGET")?;
    let mut args = read_args(parsed);
    args.insert("target".into(), Value::from(target));
    if let Some(as_of) = parsed.value("as-of") {
        args.insert("as_of".into(), Value::from(as_of));
    }
    if let Some(axis) = parsed.value("time-axis") {
        args.insert("time_axis".into(), Value::from(validate_time_axis(axis)?));
    }
    Ok(("temporal_history", Value::Object(args)))
}

fn render_task_get(payload: &Value) -> String {
    let mut lines = Vec::new();
    match payload.get("record") {
        Some(Value::Object(record)) => {
            lines.push(format!(
                "task_id: {}",
                record.get("task_id").and_then(Value::as_str).unwrap_or("?")
            ));
            lines.push(format!(
                "version: {}",
                record.get("version").and_then(Value::as_u64).unwrap_or(0)
            ));
            lines.push(format!(
                "status: {}",
                record.get("status").and_then(Value::as_str).unwrap_or("?")
            ));
        }
        _ => lines.push("no task".to_owned()),
    }
    if let Some(gist) = payload.get("ancestor_gist").and_then(Value::as_str) {
        lines.push(format!("ancestor_gist: {gist}"));
    }
    lines.join("\n")
}

fn render_task_history(payload: &Value) -> String {
    let records = payload.get("records").and_then(Value::as_array);
    let returned = records.map_or(0, Vec::len);
    let limit = payload.get("limit").and_then(Value::as_u64).unwrap_or(0);
    let mut lines = vec![format!("records: {returned} of {limit}")];
    if let Some(records) = records {
        for record in records {
            lines.push(format!(
                "{} v{} {}",
                record.get("task_id").and_then(Value::as_str).unwrap_or("?"),
                record.get("version").and_then(Value::as_u64).unwrap_or(0),
                record.get("status").and_then(Value::as_str).unwrap_or("?")
            ));
        }
    }
    lines.join("\n")
}

fn render_failure_list(payload: &Value) -> String {
    let failures = payload.get("failures").and_then(Value::as_array);
    let returned = failures.map_or(0, Vec::len);
    let limit = payload.get("limit").and_then(Value::as_u64).unwrap_or(0);
    let mode = payload
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut lines = vec![
        format!("mode: {mode}"),
        format!("failures: {returned} of {limit}"),
    ];
    if let Some(failures) = failures {
        for failure in failures {
            lines.push(format!(
                "{} attempt {}",
                failure
                    .get("task_id")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                failure
                    .get("attempt_n")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            ));
        }
    }
    lines.join("\n")
}

fn render_node_list(key: &str, payload: &Value) -> String {
    let nodes = payload.get(key).and_then(Value::as_array);
    let returned = nodes.map_or(0, Vec::len);
    let mut lines = vec![format!("{key}: {returned}")];
    if let Some(nodes) = nodes {
        for node in nodes {
            lines.push(
                node.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_owned(),
            );
        }
    }
    lines.join("\n")
}

fn render_node(payload: &Value) -> String {
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("?");
    let depth = payload.get("depth").and_then(Value::as_u64).unwrap_or(0);
    let mut lines = vec![format!("id: {id}"), format!("depth: {depth}")];
    if let Some(gist) = payload.get("summary_gist").and_then(Value::as_str) {
        lines.push(format!("summary_gist: {gist}"));
    }
    lines.join("\n")
}

fn render_temporal(payload: &Value) -> String {
    let kind = payload
        .get("target_kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let entries = payload
        .get("entries")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    format!("target_kind: {kind}\nentries: {entries}")
}

#[cfg(test)]
#[path = "cli_read_history_tests.rs"]
mod tests;
