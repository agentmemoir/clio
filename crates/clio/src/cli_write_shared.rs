// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` shared-surface and reviewer
//! commands: `shared store|retrieve|discard` (cross-agent shared bank, behind
//! the `--shared-bank` opt-in) and `validate` (collaborative review actions).
//!
//! ## Owns
//! - Flag decoding for these verbs into MCP tool arguments
//! - Their human text views
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - Shared-bank routing or reviewer semantics (see `clio-mcp`, `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to these commands' flags or arguments should normally require
//! changes here. Changes to shared-bank gating or review semantics should not
//! require changes here.
//!
//! Keep this module focused on the shared-surface and reviewer commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{base_args, one_positional, parse_unit_f64, validate_choice};
use crate::cli_write_core::caller_item_id;
use crate::cli_write_group::{Refusal, WriteCall, WriteGroup, decision_refusal};
use crate::cli_write_source_type::source_type_arg;
use serde_json::{Value, json};

/// The shared-surface and reviewer commands.
pub(crate) struct SharedWriteGroup;

impl WriteGroup for SharedWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &[
            "shared store",
            "shared retrieve",
            "shared discard",
            "validate",
        ]
    }

    fn spec(&self, verb: &str) -> FlagSpec {
        match verb {
            "shared store" => FlagSpec::new(
                &[
                    "category",
                    "episodic-type",
                    "epistemic-kind",
                    "source-type",
                    "confidence",
                    "evidence-ref",
                    "source-text",
                ],
                &["shared-bank"],
            ),
            "shared retrieve" => FlagSpec::new(&[], &["shared-bank"]),
            "shared discard" => FlagSpec::new(&["reason"], &["confirm", "shared-bank"]),
            _ => FlagSpec::new(&["action", "note", "new-content"], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        match verb {
            "shared store" => shared_store_call(parsed),
            "shared retrieve" => shared_retrieve_call(parsed),
            "shared discard" => shared_discard_call(parsed),
            _ => validate_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, palette: crate::cli_style::Palette) -> String {
        match verb {
            "shared store" => render_shared_stored(payload),
            "shared retrieve" => crate::cli_read_render::render_text("recall", payload, palette),
            "shared discard" => render_discard(payload),
            _ => render_validated(payload),
        }
    }

    fn mutates(&self, verb: &str) -> bool {
        verb == "shared store" || verb == "validate"
    }

    fn refusal(&self, verb: &str, payload: &Value) -> Option<Refusal> {
        // `shared store` returns the same gated store decision as `remember`:
        // a `pass: false` decision maps to the same exit-1 refusal the MCP path
        // reports. The other payloads carry no admission decision.
        if verb == "shared store" {
            decision_refusal(payload)
        } else {
            None
        }
    }
}

/// `shared store TEXT --epistemic-kind K [--category C] ... --shared-bank` →
/// `shared_store`: one positional becomes the gist; the tool requires explicit
/// `actor` and forces the bank to `shared`.
fn shared_store_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let text = one_positional(parsed, "shared store", "TEXT")?;
    let kind = parsed.value("epistemic-kind").ok_or_else(|| {
        CliError::usage_hint(
            "`shared store` requires --epistemic-kind (no inference)",
            "try `clio shared store \"...\" --epistemic-kind fact --actor agent --shared-bank`",
        )
    })?;
    let kind = validate_choice("epistemic-kind", kind, &["fact", "belief"])?;
    let source_type = source_type_arg(kind, parsed)?;
    let mut args = base_args(parsed);
    args.insert("item".into(), json!({"id": caller_item_id(), "gist": text}));
    args.insert("epistemic_kind".into(), Value::from(kind));
    if let Some(category) = parsed.value("category") {
        args.insert("category".into(), Value::from(category));
    }
    if let Some(episodic) = parsed.value("episodic-type") {
        args.insert("episodic_type".into(), Value::from(episodic));
    }
    if let Some(source) = source_type {
        args.insert("source_type".into(), Value::from(source));
    }
    if let Some(raw) = parsed.value("confidence") {
        args.insert(
            "confidence".into(),
            Value::from(parse_unit_f64("confidence", raw)?),
        );
    }
    if let Some(evidence) = parsed.value("evidence-ref") {
        args.insert("evidence_ref".into(), Value::from(evidence));
    }
    if let Some(haystack) = parsed.value("source-text") {
        args.insert("source_text".into(), Value::from(haystack));
    }
    Ok(WriteCall::Tool {
        tool: "shared_store",
        args: Value::Object(args),
    })
}

/// `shared retrieve QUERY --shared-bank` → `shared_retrieve`: hybrid search
/// scoped to the shared bank.
fn shared_retrieve_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let query = one_positional(parsed, "shared retrieve", "QUERY")?;
    let mut args = base_args(parsed);
    args.insert("query".into(), Value::from(query));
    Ok(WriteCall::Tool {
        tool: "shared_retrieve",
        args: Value::Object(args),
    })
}

/// `shared discard ITEM_ID --reason R [--confirm] --shared-bank` →
/// `shared_discard`: without `--confirm` the tool previews with zero writes.
fn shared_discard_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let item_id = one_positional(parsed, "shared discard", "ITEM_ID")?;
    let reason = parsed.value("reason").ok_or_else(|| {
        CliError::usage_hint(
            "`shared discard` requires --reason",
            "try `clio shared discard <ITEM_ID> --reason retired --confirm --actor agent --shared-bank`",
        )
    })?;
    let mut args = base_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    args.insert("reason".into(), Value::from(reason));
    if parsed.has("confirm") {
        args.insert("confirm".into(), Value::Bool(true));
    }
    Ok(WriteCall::Tool {
        tool: "shared_discard",
        args: Value::Object(args),
    })
}

/// `validate ITEM_ID --action A [--note N] [--new-content C]`: the tool owns
/// the non-empty-note requirement for update/invalidate/delete (exit 1 there).
fn validate_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let item_id = one_positional(parsed, "validate", "ITEM_ID")?;
    let action = parsed.value("action").ok_or_else(|| {
        CliError::usage_hint(
            "`validate` requires --action",
            "try `clio validate <ITEM_ID> --action attest`",
        )
    })?;
    let action = validate_choice(
        "action",
        action,
        &["attest", "update", "invalidate", "delete"],
    )?;
    let mut args = base_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    args.insert("action".into(), Value::from(action));
    if let Some(note) = parsed.value("note") {
        args.insert("note".into(), Value::from(note));
    }
    if let Some(content) = parsed.value("new-content") {
        args.insert("new_content".into(), Value::from(content));
    }
    Ok(WriteCall::Tool {
        tool: "validate",
        args: Value::Object(args),
    })
}

fn render_shared_stored(payload: &Value) -> String {
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("?");
    match payload.get("admission_score").and_then(Value::as_f64) {
        Some(score) => format!("stored shared item {id} (admission_score {score})"),
        None => format!("stored shared item {id}"),
    }
}

/// The discard tool previews without `confirm` (`would_discard` identity) and
/// reports removal with `discarded`; the view must match what actually happened.
fn render_discard(payload: &Value) -> String {
    let item_id = |key: &str| {
        payload
            .get(key)
            .and_then(|identity| identity.get("item_id"))
            .and_then(Value::as_str)
            .unwrap_or("?")
    };
    match payload.get("confirm").and_then(Value::as_bool) {
        Some(true) => format!("discarded {}", item_id("discarded")),
        _ => format!(
            "preview: would discard {} (pass --confirm)",
            item_id("would_discard")
        ),
    }
}

fn render_validated(payload: &Value) -> String {
    let item_id = payload
        .get("item_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let action = payload.get("action").and_then(Value::as_str).unwrap_or("?");
    let applied = payload
        .get("applied")
        .map_or_else(|| "?".to_owned(), ToString::to_string);
    format!("validated {item_id} action={action} applied={applied}")
}

#[cfg(test)]
#[path = "cli_write_shared_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_write_shared_validate_tests.rs"]
mod cli_write_shared_validate_tests;
