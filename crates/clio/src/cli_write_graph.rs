// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` triple/belief/graph write
//! commands: `triple add`, `triple end`, `belief observe`, and `graph link`,
//! decoded into their gated MCP tools with `--dry-run` no-write previews.
//!
//! ## Owns
//! - Flag decoding for these write verbs into MCP tool arguments
//! - Their `--dry-run` no-write preview payloads (context redacted to a
//!   presence/length summary) and their human text views
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - The MCP tool semantics or gating (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to these write commands' flags or arguments should normally require
//! changes here. Changes to the write engine should not require changes here.
//!
//! Keep this module focused on the triple/belief/graph write commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    base_args, flag, one_positional, parse_f64, require_flags, required_flags, validate_choice,
};
use crate::cli_write_core::context_arg;
use crate::cli_write_group::{WriteCall, WriteGroup};
use serde_json::{Map, Value, json};

/// The triple/belief/graph write commands.
pub(crate) struct GraphWriteGroup;

impl WriteGroup for GraphWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["triple add", "triple end", "belief observe", "graph link"]
    }

    fn spec(&self, verb: &str) -> FlagSpec {
        match verb {
            "triple add" => FlagSpec::new(
                &[
                    "epistemic-kind",
                    "source-type",
                    "confidence",
                    "context",
                    "valid-from",
                    "valid-until",
                    "supersede",
                    "subject-id",
                    "update-rule",
                ],
                &["dry-run"],
            ),
            "triple end" => FlagSpec::new(&["object", "valid-until", "update-rule"], &["dry-run"]),
            "belief observe" => FlagSpec::new(
                &[
                    "confidence",
                    "evidence",
                    "source",
                    "as-of",
                    "subject-id",
                    "context",
                ],
                &["dry-run"],
            ),
            _ => FlagSpec::new(&["relationship", "weight"], &["dry-run"]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        match verb {
            "triple add" => triple_add_call(verb, parsed),
            "triple end" => triple_end_call(verb, parsed),
            "belief observe" => belief_observe_call(verb, parsed),
            _ => graph_link_call(verb, parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "triple add" => render_triple_add(payload),
            "triple end" => render_triple_end(payload),
            "belief observe" => render_belief(payload),
            _ => render_graph_link(payload),
        }
    }

    fn mutates(&self, _verb: &str) -> bool {
        true
    }
}

/// `triple add S P O [--epistemic-kind K] [--dry-run]` → `triple_add` (gated
/// episodic admission; supersede closes the prior open subject+predicate).
fn triple_add_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    if positionals.len() != 3 {
        return Err(positional_error(
            verb,
            &["SUBJECT", "PREDICATE", "OBJECT"],
            positionals.len(),
        ));
    }
    let kind = match parsed.value("epistemic-kind") {
        Some(raw) => validate_choice("epistemic-kind", raw, &["fact", "belief"])?,
        None => "fact",
    };
    let mut args = base_args(parsed);
    args.insert("subject".into(), Value::from(positionals[0].as_str()));
    args.insert("predicate".into(), Value::from(positionals[1].as_str()));
    args.insert("object".into(), Value::from(positionals[2].as_str()));
    args.insert("epistemic_kind".into(), Value::from(kind));
    if let Some(source) = parsed.value("source-type") {
        args.insert("source_type".into(), Value::from(source));
    }
    if let Some(raw) = parsed.value("confidence") {
        args.insert(
            "confidence".into(),
            Value::from(parse_f64("confidence", raw)?),
        );
    }
    if let Some(ts) = parsed.value("valid-from") {
        args.insert("valid_from".into(), Value::from(ts));
    }
    if let Some(ts) = parsed.value("valid-until") {
        args.insert("valid_until".into(), Value::from(ts));
    }
    if let Some(raw) = parsed.value("supersede") {
        args.insert(
            "supersede".into(),
            Value::Bool(validate_choice("supersede", raw, &["true", "false"])? == "true"),
        );
    }
    if let Some(subject) = parsed.value("subject-id") {
        args.insert("subject_id".into(), Value::from(subject));
    }
    if let Some(rule) = parsed.value("update-rule") {
        args.insert("update_rule".into(), Value::from(rule));
    }
    if let Some(context) = context_arg(parsed)? {
        args.insert("context".into(), Value::from(context));
    }
    if parsed.has("dry-run") {
        return Ok(dry_run_preview(&args, "write a gated bi-temporal SPO edge"));
    }
    Ok(WriteCall::Tool {
        tool: "triple_add",
        args: Value::Object(args),
    })
}

/// `triple end S P
///  [--object O] --valid-until TS` → `triple_end` (expire open
/// edges without replacement; history is retained).
fn triple_end_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    if positionals.len() != 2 {
        return Err(CliError::usage_hint(
            format!("`{verb}` requires SUBJECT and PREDICATE arguments"),
            format!("usage: clio {verb} SUBJECT PREDICATE --valid-until TS"),
        ));
    }
    require_flags(parsed, verb, required_flags(verb))?;
    let valid_until = flag(parsed, "valid-until");
    let mut args = base_args(parsed);
    args.insert("subject".into(), Value::from(positionals[0].as_str()));
    args.insert("predicate".into(), Value::from(positionals[1].as_str()));
    args.insert("valid_until".into(), Value::from(valid_until));
    if let Some(object) = parsed.value("object") {
        args.insert("object".into(), Value::from(object));
    }
    if let Some(rule) = parsed.value("update-rule") {
        args.insert("update_rule".into(), Value::from(rule));
    }
    if parsed.has("dry-run") {
        return Ok(dry_run_preview(
            &args,
            "expire the matching open SPO edges; history is retained",
        ));
    }
    Ok(WriteCall::Tool {
        tool: "triple_end",
        args: Value::Object(args),
    })
}

/// `belief observe PROP --confidence C --evidence E --source S` →
/// `belief_observe`: gated create for a new identity, validated append (no
/// novelty re-score) for an existing one.
fn belief_observe_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let proposition = one_positional(parsed, verb, "PROPOSITION")?;
    let mut args = base_args(parsed);
    args.insert("proposition".into(), Value::from(proposition));
    require_flags(parsed, verb, required_flags(verb))?;
    let confidence = flag(parsed, "confidence");
    args.insert(
        "confidence".into(),
        Value::from(parse_f64("confidence", confidence)?),
    );
    args.insert("evidence_ref".into(), Value::from(flag(parsed, "evidence")));
    args.insert("source_type".into(), Value::from(flag(parsed, "source")));
    if let Some(as_of) = parsed.value("as-of") {
        args.insert("as_of".into(), Value::from(as_of));
    }
    if let Some(subject) = parsed.value("subject-id") {
        args.insert("subject_id".into(), Value::from(subject));
    }
    if let Some(context) = context_arg(parsed)? {
        args.insert("context".into(), Value::from(context));
    }
    if parsed.has("dry-run") {
        return Ok(dry_run_preview(
            &args,
            "create the belief under five-factor admission, or append to the existing identity; history is append-only",
        ));
    }
    Ok(WriteCall::Tool {
        tool: "belief_observe",
        args: Value::Object(args),
    })
}

/// `graph link A B --relationship R [--weight W]` → `graph_link` (explicit
/// assoc edge; identity includes the relationship label).
fn graph_link_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    if positionals.len() != 2 {
        return Err(CliError::usage_hint(
            format!("`{verb}` requires SOURCE and TARGET item ids"),
            format!("usage: clio {verb} A B --relationship R"),
        ));
    }
    require_flags(parsed, verb, required_flags(verb))?;
    let mut args = base_args(parsed);
    args.insert("source_id".into(), Value::from(positionals[0].as_str()));
    args.insert("target_id".into(), Value::from(positionals[1].as_str()));
    args.insert(
        "relationship".into(),
        Value::from(flag(parsed, "relationship")),
    );
    if let Some(raw) = parsed.value("weight") {
        args.insert("weight".into(), Value::from(parse_f64("weight", raw)?));
    }
    if parsed.has("dry-run") {
        return Ok(dry_run_preview(
            &args,
            "declare an explicit association edge (sticky); no existing memory is altered",
        ));
    }
    Ok(WriteCall::Tool {
        tool: "graph_link",
        args: Value::Object(args),
    })
}

fn positional_error(verb: &str, labels: &[&str], got: usize) -> CliError {
    CliError::usage_hint(
        format!(
            "`{verb}` requires {} arguments ({})",
            labels.len(),
            labels.join(" ")
        ),
        format!(
            "got {got} positional argument(s); usage: clio {verb} {}",
            labels.join(" ")
        ),
    )
}

/// The `--dry-run` preview payload. Raw `context` is never echoed: the redacted
/// arguments carry only its presence and UTF-8 byte length, so a secret-shaped
/// value never reaches preview JSON, terminals, or logs.
fn dry_run_preview(args: &Map<String, Value>, would: &str) -> WriteCall {
    let mut redacted = args.clone();
    if let Some(context) = args.get("context").and_then(Value::as_str) {
        redacted.insert(
            "context".into(),
            json!({"present": true, "bytes": context.len()}),
        );
    } else {
        redacted.remove("context");
    }
    let payload = json!({
        "dry_run": true,
        "arguments": redacted,
        "would": would,
    });
    let text = format!("dry-run: would {would}");
    WriteCall::Preview { payload, text }
}

fn render_triple_add(payload: &Value) -> String {
    match payload.get("new_edge_id").and_then(Value::as_str) {
        Some(edge) => {
            let prior = payload
                .get("prior_edge_ids")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("edge {edge} (closed {prior} prior edge(s))")
        }
        None => "edge accepted".to_owned(),
    }
}

fn render_triple_end(payload: &Value) -> String {
    let closed = payload
        .get("closed_ids")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let already = payload
        .get("already_ended")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!("closed {closed} edge(s), already_ended {already}")
}

fn render_belief(payload: &Value) -> String {
    match payload.get("outcome").and_then(Value::as_str) {
        Some("created") => {
            let id = payload
                .pointer("/belief/id")
                .and_then(Value::as_str)
                .unwrap_or("?");
            format!("belief {id} created")
        }
        Some("appended") => {
            let id = payload
                .get("belief_id")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let count = payload
                .get("entry_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            format!("belief {id} appended (entry_count {count})")
        }
        _ => "belief observed".to_owned(),
    }
}

fn render_graph_link(payload: &Value) -> String {
    let edge = payload
        .get("edge_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let source = payload
        .get("source_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let target = payload
        .get("target_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    format!("linked {source} -> {target} (edge {edge})")
}

#[cfg(test)]
#[path = "cli_write_graph_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_write_graph_flag_tests.rs"]
mod flag_tests;

#[cfg(test)]
#[path = "cli_write_graph_context_tests.rs"]
mod cli_write_graph_context_tests;
