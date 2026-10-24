// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` confirmed memory-mutation commands
//! `update`, `invalidate`, `discard`, and `correct`, decoded into their gated
//! MCP tools with `--dry-run` no-write previews.
//!
//! ## Owns
//! - Flag decoding for these mutation verbs into MCP tool arguments (1:1 with the
//!   published `inputSchema`)
//! - The `discard` confirmation gate call and the blast-radius dry-run preview
//! - Their human text views
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - The confirmation decision itself (see `cli_confirm`)
//! - Mutation semantics; the in-process tools own behavior (see `clio-write`,
//!   `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to these commands' flags or arguments should normally require changes
//! here. Changes to the write engine or the confirm gate should not.
//!
//! Keep this module focused on the mutation write commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    base_args, flag, one_positional, require_flags, required_flags, validate_choice,
};
use crate::cli_write_core::context_arg;
use crate::cli_write_group::{WriteCall, WriteGroup};
use serde_json::{Map, Value, json};

/// The confirmed memory-mutation commands.
pub(crate) struct MutationWriteGroup;

impl WriteGroup for MutationWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["update", "invalidate", "discard", "correct"]
    }

    // The registry record owns each verb's accepted set, so the parser and the
    // usage line cannot disagree; a name with no record accepts only the globals.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(record) => record.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        match verb {
            "update" => update_call(parsed),
            "invalidate" => invalidate_call(parsed),
            "discard" => discard_call(parsed),
            _ => correct_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "update" => render_update(payload),
            "invalidate" => render_invalidate(payload),
            "discard" => render_discard(payload),
            _ => render_correct(payload),
        }
    }

    fn mutates(&self, _verb: &str) -> bool {
        true
    }
}

/// `update ID VALUE --rule discrete|continuous` → `update`: the tool routes
/// `discrete` to supersession, which stores the value on a replacement item and
/// returns its id, and `continuous` to the shared EMA engine.
fn update_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    if positionals.len() != 2 {
        return Err(positional_error(
            "update",
            &["ID", "VALUE"],
            positionals.len(),
        ));
    }
    require_flags(parsed, "update", required_flags("update"))?;
    let rule = flag(parsed, "rule");
    let rule = validate_choice("rule", rule, &["discrete", "continuous"])?;
    let mut args = base_args(parsed);
    args.insert("target".into(), Value::from(positionals[0].as_str()));
    args.insert("new_value".into(), Value::from(positionals[1].as_str()));
    args.insert("update_rule".into(), Value::from(rule));
    if parsed.has("dry-run") {
        return Ok(dry_run_preview(
            "update",
            positionals[0].as_str(),
            parsed,
            &args,
            "apply the selected rule: `discrete` supersedes the item and stores the value on a \
replacement item whose id is returned, `continuous` runs the shared EMA engine",
        ));
    }
    Ok(WriteCall::Tool {
        tool: "update",
        args: Value::Object(args),
    })
}

/// `invalidate ID [--replacement ID]` → `invalidate`: close validity on both
/// axes, optionally chaining a replacement; never deletes.
fn invalidate_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let item_id = one_positional(parsed, "invalidate", "ID")?;
    let mut args = base_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    if let Some(replacement) = parsed.value("replacement") {
        args.insert("replacement_id".into(), Value::from(replacement));
    }
    if parsed.has("dry-run") {
        return Ok(dry_run_preview(
            "invalidate",
            item_id,
            parsed,
            &args,
            "close the item's validity on both time axes; nothing is deleted and history stays queryable",
        ));
    }
    Ok(WriteCall::Tool {
        tool: "invalidate",
        args: Value::Object(args),
    })
}

/// `discard ID --reason R --confirm` → `discard`: logged operations removal.
///
/// The shared confirm gate refuses without `--confirm`/`--yes` (exit 2) and
/// `--dry-run` prints the blast radius with zero writes. Discard is never
/// compliance erasure; the item's key and content are untouched.
fn discard_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let item_id = one_positional(parsed, "discard", "ID")?;
    require_flags(parsed, "discard", required_flags("discard"))?;
    let reason = flag(parsed, "reason");
    if parsed.has("dry-run") {
        let scope = parsed.value("bank").unwrap_or("default");
        return Ok(WriteCall::Preview {
            payload: json!({
                "dry_run": true,
                "operation": "discard",
                "target": item_id,
                "scope": scope,
                "would": "set discarded_at/discard_reason (logged operations removal)",
                "irreversible": false,
            }),
            text: format!(
                "dry-run: would discard {item_id} in bank `{scope}`\n  effect: sets discarded_at/discard_reason (logged removal from active use)\n  note: discard is not compliance erasure and does not destroy the item's key\n  pass --confirm to apply"
            ),
        });
    }
    crate::cli_confirm::require_confirm("discard", parsed)?;
    let mut args = base_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    args.insert("reason".into(), Value::from(reason));
    args.insert("confirm".into(), Value::Bool(true));
    Ok(WriteCall::Tool {
        tool: "discard",
        args: Value::Object(args),
    })
}

/// `correct ID VALUE --reason R [--context C]` → `correct`: an attributable
/// correction that preserves history; `--context C` explicitly replaces the
/// item's bounded source context. The CLI invocation is itself the operator
/// confirmation, so it runs the confirmed path unless `--dry-run` asks for the
/// zero-write preview.
fn correct_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    if positionals.len() != 2 {
        return Err(positional_error(
            "correct",
            &["ID", "VALUE"],
            positionals.len(),
        ));
    }
    require_flags(parsed, "correct", required_flags("correct"))?;
    let reason = flag(parsed, "reason");
    let context = context_arg(parsed)?;
    let item_id = positionals[0].as_str();
    let new_value = positionals[1].as_str();
    if parsed.has("dry-run") {
        return Ok(WriteCall::Preview {
            payload: json!({
                "dry_run": true,
                "operation": "correct",
                "confirm": false,
                "would_correct": {
                    "item_id": item_id,
                    "new_value": new_value,
                    "reason": reason,
                },
            }),
            text: format!(
                "dry-run: would correct {item_id} (prior revision superseded, never deleted)"
            ),
        });
    }
    let mut args = base_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    args.insert("new_value".into(), Value::from(new_value));
    args.insert("reason".into(), Value::from(reason));
    args.insert("confirm".into(), Value::Bool(true));
    if let Some(context) = context {
        args.insert("context".into(), Value::from(context));
    }
    Ok(WriteCall::Tool {
        tool: "correct",
        args: Value::Object(args),
    })
}

/// A no-write preview for a mutation whose in-process tool has no dry-run path:
/// it names the operation, the target, the bank scope, and what would change.
fn dry_run_preview(
    operation: &str,
    target: &str,
    parsed: &ParsedArgs,
    args: &Map<String, Value>,
    would: &str,
) -> WriteCall {
    let scope = parsed.value("bank").unwrap_or("default");
    let payload = json!({
        "dry_run": true,
        "operation": operation,
        "target": target,
        "scope": scope,
        "arguments": args,
        "would": would,
    });
    WriteCall::Preview {
        payload,
        text: format!("dry-run: would {operation} {target} in bank `{scope}`\n  effect: {would}"),
    }
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

fn render_update(payload: &Value) -> String {
    match payload.get("rule").and_then(Value::as_str) {
        Some("continuous") => match payload.pointer("/state/state").and_then(Value::as_f64) {
            Some(state) => format!("updated via EMA (state {state})"),
            None => "updated via EMA".to_owned(),
        },
        Some("discrete") => {
            let closed = payload
                .get("closed_ids")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            match payload.get("replacement_id").and_then(Value::as_str) {
                Some(replacement) => {
                    format!("updated (discrete): closed {closed}, replacement {replacement}")
                }
                None => format!("updated (discrete): closed {closed}"),
            }
        }
        _ => "updated".to_owned(),
    }
}

fn render_invalidate(payload: &Value) -> String {
    let closed = payload
        .get("closed_ids")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let already = payload
        .get("already_ended")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    match payload.get("replacement_id").and_then(Value::as_str) {
        Some(replacement) => {
            format!(
                "invalidated (closed {closed}, already_ended {already}, replacement {replacement})"
            )
        }
        None => format!("invalidated (closed {closed}, already_ended {already})"),
    }
}

fn render_discard(payload: &Value) -> String {
    let item = payload
        .pointer("/discarded/item_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    format!("discarded {item}")
}

fn render_correct(payload: &Value) -> String {
    match payload.get("rule").and_then(Value::as_str) {
        Some("continuous") => "corrected via EMA (history preserved)".to_owned(),
        Some("discrete") => {
            let new_id = payload
                .pointer("/correction/new_item_id")
                .and_then(Value::as_str)
                .unwrap_or("?");
            format!("corrected (new revision {new_id}; prior superseded, not deleted)")
        }
        _ => "corrected".to_owned(),
    }
}

#[cfg(test)]
#[path = "cli_write_mutate_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_write_mutate_text_tests.rs"]
mod text_tests;
