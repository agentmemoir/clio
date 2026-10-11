// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` workspace write commands:
//! `scratchpad write|read|clear` (ephemeral only) and `canonical put|get`
//! (gated single-slot upsert and its read).
//!
//! ## Owns
//! - Flag decoding for these verbs into MCP tool arguments
//! - Their human text views
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - Scratchpad quotas or canonical supersession (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to these commands' flags or arguments should normally require
//! changes here. Changes to scratchpad or canonical semantics should not
//! require changes here.
//!
//! Keep this module focused on the scratchpad and canonical commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    base_args, flag, one_positional, parse_unit_f64, require_flags, required_flags, validate_choice,
};
use crate::cli_write_core::caller_item_id;
use crate::cli_write_core::context_arg;
use crate::cli_write_group::{Refusal, WriteCall, WriteGroup, decision_refusal};
use crate::cli_write_source_type::source_type_arg;
use serde_json::{Map, Value, json};

/// The scratchpad and canonical commands.
pub(crate) struct WorkspaceWriteGroup;

impl WriteGroup for WorkspaceWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &[
            "scratchpad write",
            "scratchpad read",
            "scratchpad clear",
            "canonical put",
            "canonical get",
        ]
    }

    fn spec(&self, verb: &str) -> FlagSpec {
        match verb {
            "canonical put" => FlagSpec::new(
                &[
                    "category",
                    "epistemic-kind",
                    "source-type",
                    "confidence",
                    "source-text",
                    "evidence-ref",
                    "context",
                ],
                &["dry-run"],
            ),
            "canonical get" => FlagSpec::new(&["as-of"], &[]),
            _ => FlagSpec::new(&["session"], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        match verb {
            "scratchpad write" => scratchpad_write_call(verb, parsed),
            "scratchpad read" => scratchpad_key_call("scratchpad_read", parsed),
            "scratchpad clear" => scratchpad_key_call("scratchpad_clear", parsed),
            "canonical put" => canonical_put_call(verb, parsed),
            _ => canonical_get_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "scratchpad write" => render_scratchpad_write(payload),
            "scratchpad read" => render_scratchpad_read(payload),
            "scratchpad clear" => "cleared".to_owned(),
            "canonical put" => render_canonical_put(payload),
            _ => render_canonical_get(payload),
        }
    }

    fn mutates(&self, verb: &str) -> bool {
        verb == "canonical put"
    }

    fn refusal(&self, _verb: &str, payload: &Value) -> Option<Refusal> {
        // A `--dry-run` payload is a scored preview (mirrors `admit`): it is
        // the expected output even when the decision rejects, so it never maps
        // to the exit-1 refusal. A real `canonical put` returns the decision
        // flattened at top level, which `decision_refusal` handles.
        if payload.get("dry_run").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        decision_refusal(payload)
    }
}

/// `scratchpad write KEY CONTENT [--session S]` → `scratchpad_write` (ephemeral,
/// quota-bounded; never durable).
fn scratchpad_write_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    if positionals.len() > 2 {
        return Err(unexpected_extra(verb, parsed, "KEY CONTENT"));
    }
    let [key, content] = positionals else {
        return Err(CliError::usage_hint(
            format!("`{verb}` requires KEY and CONTENT arguments"),
            format!("usage: clio {verb} KEY CONTENT"),
        ));
    };
    let mut args = base_args(parsed);
    args.insert("key".into(), Value::from(key.as_str()));
    args.insert("content".into(), Value::from(content.as_str()));
    insert_session(parsed, &mut args);
    Ok(WriteCall::Tool {
        tool: "scratchpad_write",
        args: Value::Object(args),
    })
}

/// `scratchpad read|clear [KEY] [--session S]`: no positional reads or purges
/// the whole session pad; exactly one names a key (missing keys error in the
/// tool).
fn scratchpad_key_call(tool: &'static str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let key = match parsed.positionals() {
        [] => None,
        [only] => Some(only.as_str()),
        _ => return Err(unexpected_extra(tool, parsed, "[KEY]")),
    };
    let mut args = base_args(parsed);
    if let Some(key) = key {
        args.insert("key".into(), Value::from(key));
    }
    insert_session(parsed, &mut args);
    Ok(WriteCall::Tool {
        tool,
        args: Value::Object(args),
    })
}

/// `canonical put KEY VALUE --category C --epistemic-kind K [...] [--dry-run]` →
/// `canonical_put` (gated single-slot upsert with bi-temporal supersession).
fn canonical_put_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    if positionals.len() != 2 {
        return Err(CliError::usage_hint(
            format!("`{verb}` requires KEY and VALUE arguments"),
            format!(
                "got {} positional argument(s); usage: clio {verb} KEY VALUE --category C --epistemic-kind K",
                positionals.len()
            ),
        ));
    }
    require_flags(parsed, verb, required_flags(verb))?;
    let kind = validate_choice(
        "epistemic-kind",
        flag(parsed, "epistemic-kind"),
        &["fact", "belief"],
    )?;
    let source_type = source_type_arg(kind, parsed)?;
    let mut args = base_args(parsed);
    args.insert("key".into(), Value::from(positionals[0].as_str()));
    args.insert("value".into(), Value::from(positionals[1].as_str()));
    args.insert("category".into(), Value::from(flag(parsed, "category")));
    args.insert("epistemic_kind".into(), Value::from(kind));
    if let Some(source) = source_type {
        args.insert("source_type".into(), Value::from(source));
    }
    if let Some(raw) = parsed.value("confidence") {
        args.insert(
            "confidence".into(),
            Value::from(parse_unit_f64("confidence", raw)?),
        );
    }
    if let Some(text) = parsed.value("source-text") {
        args.insert("source_text".into(), Value::from(text));
    }
    if let Some(evidence) = parsed.value("evidence-ref") {
        args.insert("evidence_ref".into(), Value::from(evidence));
    }
    if let Some(context) = context_arg(parsed)? {
        args.insert("context".into(), Value::from(context));
    }
    if parsed.has("dry-run") {
        args.insert("dry_run".into(), Value::Bool(true));
    }
    // One-shot CLI processes must not reuse the resettable in-process id
    // counter (mirrors `remember`).
    args.insert("item".into(), json!({"id": caller_item_id()}));
    Ok(WriteCall::Tool {
        tool: "canonical_put",
        args: Value::Object(args),
    })
}

/// `canonical get KEY [--as-of TS]` → `canonical_get` (active or point-in-time
/// slot value; an empty slot errors in the tool).
fn canonical_get_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let key = one_positional(parsed, "canonical get", "KEY")?;
    let mut args = base_args(parsed);
    args.insert("key".into(), Value::from(key));
    if let Some(as_of) = parsed.value("as-of") {
        args.insert("as_of".into(), Value::from(as_of));
    }
    Ok(WriteCall::Tool {
        tool: "canonical_get",
        args: Value::Object(args),
    })
}

fn insert_session(parsed: &ParsedArgs, args: &mut Map<String, Value>) {
    if let Some(session) = parsed.value("session") {
        args.insert("session".into(), Value::from(session));
    }
}

fn unexpected_extra(verb: &str, parsed: &ParsedArgs, accepts: &str) -> CliError {
    crate::cli_read_args::unexpected_extra(parsed, 1, Some(verb), accepts)
}

fn render_scratchpad_write(payload: &Value) -> String {
    let key = payload.get("key").and_then(Value::as_str).unwrap_or("?");
    let bytes = payload.get("bytes").and_then(Value::as_u64).unwrap_or(0);
    format!("scratchpad `{key}` stored for this process only ({bytes} bytes)")
}

/// One key renders its content exactly; a whole-pad read renders summary counts.
fn render_scratchpad_read(payload: &Value) -> String {
    if let Some(content) = payload.get("content") {
        return match content.as_str() {
            Some(text) => text.to_owned(),
            None => serde_json::to_string(content).unwrap_or_else(|_| content.to_string()),
        };
    }
    let entries = payload.get("entries").and_then(Value::as_array);
    let total = payload
        .get("total_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!(
        "entries: {} (total_bytes: {total})",
        entries.map_or(0, Vec::len)
    )
}

/// The `canonical put` view: the dry-run decision is a preview, never a write
/// claim; a stored revision reports its item id. The tool's payload does not
/// echo the slot key (it carries the flattened admission decision), so the view
/// identifies the revision by id only.
fn render_canonical_put(payload: &Value) -> String {
    if payload.get("dry_run").and_then(Value::as_bool) == Some(true) {
        return "dry-run: would put the canonical slot (zero writes)".to_owned();
    }
    match payload.get("id").and_then(Value::as_str) {
        Some(id) => format!("stored canonical (id {id})"),
        None => "stored canonical".to_owned(),
    }
}

fn render_canonical_get(payload: &Value) -> String {
    let key = payload.get("key").and_then(Value::as_str).unwrap_or("?");
    let value = payload.get("value").and_then(Value::as_str).unwrap_or("?");
    let item_id = payload
        .get("item_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let valid_from = payload
        .get("valid_from")
        .and_then(Value::as_str)
        .unwrap_or("?");
    format!("canonical `{key}` = {value} (item_id: {item_id}, valid_from: {valid_from})")
}

#[cfg(test)]
#[path = "cli_write_workspace_tests.rs"]
mod tests;
